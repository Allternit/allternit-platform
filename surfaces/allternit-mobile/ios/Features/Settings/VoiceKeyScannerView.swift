import AVFoundation
import SwiftUI
import UIKit

/// Settings › Voice › Scan from my computer: the camera reads QR 1, shows
/// the 6-digit code, then reads QR 2 and saves the key on this phone
/// (`VoiceKeyHandoffSession`).
struct VoiceKeyHandoffSheet: View {
    @Environment(\.dismiss) private var dismiss
    @StateObject private var session = VoiceKeyHandoffSession()
    @State private var camera: AVAuthorizationStatus = AVCaptureDevice.authorizationStatus(for: .video)

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 16) {
                Text(instructions)
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)

                cameraArea
                    .frame(maxWidth: .infinity)
                    .aspectRatio(1, contentMode: .fit)
                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))

                status
                Spacer(minLength: 0)
            }
            .padding(16)
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Scan from my computer")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(isDone ? "Done" : "Cancel") {
                        session.cancel()
                        dismiss()
                    }
                }
            }
        }
        .onDisappear { session.cancel() }
        .task {
            if camera == .notDetermined {
                let granted = await AVCaptureDevice.requestAccess(for: .video)
                camera = granted ? .authorized : .denied
            }
        }
    }

    private var isDone: Bool {
        if case .done = session.phase { return true }
        return false
    }

    private var instructions: String {
        switch session.phase {
        case .confirm:
            return "Check the code matches, then press Codes match on your computer. The key arrives here by itself."
        default:
            return "On your computer, open Settings › Voice in Allternit and click Send to my phone. Then point this camera at the QR code."
        }
    }

    @ViewBuilder private var cameraArea: some View {
        switch camera {
        case .authorized:
            if case .done = session.phase {
                Color("FillNeutral")
                    .overlay(Image(systemName: "checkmark.circle").font(.system(size: 44)).foregroundColor(Color("TextPrimary")))
            } else if AVCaptureDevice.default(for: .video) == nil {
                cameraMessage("This device has no camera. Paste or type the key instead.", settingsLink: false)
            } else {
                VoiceKeyScannerView { session.scanned($0) }
            }
        case .notDetermined:
            Color("FillNeutral").overlay(ProgressView())
        default:
            cameraMessage("Camera access is off. Turn it on in Settings › Allternit to scan the code, or paste the key instead.", settingsLink: true)
        }
    }

    private func cameraMessage(_ text: String, settingsLink: Bool) -> some View {
        Color("FillNeutral").overlay(
            VStack(spacing: 12) {
                Image(systemName: "camera").font(.system(size: 28)).foregroundColor(Color("TextSecondary"))
                Text(text)
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                if settingsLink, let url = URL(string: UIApplication.openSettingsURLString) {
                    Button("Open Settings") { UIApplication.shared.open(url) }
                        .buttonStyle(SettingsQuietButtonStyle())
                }
            }
            .padding(24)
        )
    }

    @ViewBuilder private var status: some View {
        switch session.phase {
        case .scanning:
            EmptyView()
        case .registering:
            HStack(spacing: 8) {
                ProgressView()
                Text("Connecting to your computer…").font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
            }
        case .confirm(let code):
            VStack(alignment: .leading, spacing: 6) {
                Text("Check this matches your computer")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Text("\(code.prefix(3)) \(code.suffix(3))")
                    .font(.system(size: 34, weight: .semibold, design: .monospaced))
                    .foregroundColor(Color("TextPrimary"))
                    .accessibilityLabel(code.map(String.init).joined(separator: " "))
                Text("If the numbers differ, cancel. Someone else may be showing a code.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(14)
            .background(Color("FillNeutral"))
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        case .checking(let vendor):
            HStack(spacing: 8) {
                ProgressView()
                Text("Checking your \(vendor.name) key…").font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
            }
        case .done(let vendor, let label):
            VStack(alignment: .leading, spacing: 4) {
                Text("● Connected · \(label)")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Text("Your \(vendor.name) key is saved on this phone, and voice now runs on your own \(vendor.name).")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
        case .failed(let message):
            VStack(alignment: .leading, spacing: 10) {
                SettingsErrorBanner(message: message)
                Button("Try again") { session.restart() }
                    .buttonStyle(SettingsQuietButtonStyle())
            }
        }
    }
}

/// A QR scanner (AVCaptureMetadataOutput). Calls `onCode` on the main actor
/// for every QR string it reads (the same string at most every 2 seconds).
struct VoiceKeyScannerView: UIViewControllerRepresentable {
    let onCode: @MainActor (String) -> Void

    func makeUIViewController(context: Context) -> ScannerController {
        let controller = ScannerController()
        controller.onCode = onCode
        return controller
    }

    func updateUIViewController(_ controller: ScannerController, context: Context) {
        controller.onCode = onCode
    }

    static func dismantleUIViewController(_ controller: ScannerController, coordinator: ()) {
        controller.stop()
    }

    final class ScannerController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
        var onCode: (@MainActor (String) -> Void)?
        private let captureSession = AVCaptureSession()
        private var preview: AVCaptureVideoPreviewLayer?
        private var lastCode: (text: String, at: Date)?

        override func viewDidLoad() {
            super.viewDidLoad()
            view.backgroundColor = .black
            guard let device = AVCaptureDevice.default(for: .video),
                  let input = try? AVCaptureDeviceInput(device: device),
                  captureSession.canAddInput(input) else { return }
            captureSession.addInput(input)
            let output = AVCaptureMetadataOutput()
            guard captureSession.canAddOutput(output) else { return }
            captureSession.addOutput(output)
            output.setMetadataObjectsDelegate(self, queue: .main)
            output.metadataObjectTypes = [.qr]
            let layer = AVCaptureVideoPreviewLayer(session: captureSession)
            layer.videoGravity = .resizeAspectFill
            view.layer.addSublayer(layer)
            preview = layer
        }

        override func viewDidLayoutSubviews() {
            super.viewDidLayoutSubviews()
            preview?.frame = view.bounds
        }

        override func viewWillAppear(_ animated: Bool) {
            super.viewWillAppear(animated)
            nonisolated(unsafe) let session = captureSession
            DispatchQueue.global(qos: .userInitiated).async {
                if !session.isRunning { session.startRunning() }
            }
        }

        override func viewWillDisappear(_ animated: Bool) {
            super.viewWillDisappear(animated)
            stop()
        }

        func stop() {
            nonisolated(unsafe) let session = captureSession
            DispatchQueue.global(qos: .userInitiated).async {
                if session.isRunning { session.stopRunning() }
            }
        }

        nonisolated func metadataOutput(_ output: AVCaptureMetadataOutput,
                                        didOutput metadataObjects: [AVMetadataObject],
                                        from connection: AVCaptureConnection) {
            let codes = metadataObjects.compactMap { ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }
            MainActor.assumeIsolated {
                for code in codes {
                    // The camera reports a code many times a second.
                    if let lastCode, lastCode.text == code, Date().timeIntervalSince(lastCode.at) < 2 { continue }
                    lastCode = (code, Date())
                    onCode?(code)
                }
            }
        }
    }
}
