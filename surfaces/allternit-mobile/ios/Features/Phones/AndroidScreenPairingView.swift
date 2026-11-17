import CoreImage.CIFilterBuiltins
import SwiftUI
import UIKit

/// Connect kit row `android-screen` ("Android phone (screen share)"), the iOS
/// mirror of AndroidScreenPane.tsx: get a one-time code (text, QR, share
/// link), then approve the phone's pairing request. The screen is relayed by
/// one of the account's computers, so it checks for one first.
struct AndroidPairing {
    struct Claim: Equatable {
        let phoneId: String
        let code: String
        let expiresAt: Date
    }

    struct Pending: Identifiable, Equatable {
        let phoneId: String
        let name: String
        let model: String
        let code: String
        var id: String { phoneId }
    }

    enum Lane: Equatable {
        case checking
        case ready(runtimeId: String, runtimeName: String)
        case none
        case offline(String)
        case old(String)
        case signedOut
        case error(String)
    }

    static let noRuntime = "Your phone’s screen goes through one of your computers. Open Allternit Desktop on your Mac or PC, or get an Allternit Cloud computer."
    static let oldRuntime = "Update Allternit on your computer to share an Android phone’s screen."
    static let playURL = URL(string: "https://play.google.com/store/apps/details?id=com.allternit.app")!

    static func deepLink(_ code: String) -> URL? {
        var parts = URLComponents()
        parts.scheme = "allternit"
        parts.host = "phone-share"
        parts.queryItems = [URLQueryItem(name: "code", value: code)]
        return parts.url
    }

    /// `{pairs:[…]}` from the pending route; entries without a 6-digit code are dropped.
    static func readPending(_ data: Data) -> [Pending] {
        guard let root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let pairs = root["pairs"] as? [[String: Any]] else { return [] }
        return pairs.compactMap { item in
            let body = (item["data"] as? [String: Any]) ?? (item["payload"] as? [String: Any]) ?? item
            guard let phoneId = body["phoneId"] as? String else { return nil }
            let code = String(describing: body["code"] ?? "").filter(\.isNumber)
            guard code.count == 6 else { return nil }
            let platform = (item["platform"] as? String) ?? (body["platform"] as? String)
            let rawName = (body["name"] as? String)?.trimmingCharacters(in: .whitespaces) ?? ""
            let name = rawName.isEmpty || (rawName == "iPhone" && platform == "android") ? "Android phone" : rawName
            return Pending(phoneId: phoneId, name: String(name.prefix(80)), model: String((body["model"] as? String ?? "").prefix(80)), code: code)
        }
    }

    // MARK: Network

    private static func call(_ path: String, method: String = "GET", body: [String: Any]? = nil) async throws -> (Int, Data) {
        let base = AppConfig.cloudAPIBaseURL.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard let url = URL(string: base + "/" + path) else { throw ACIComputersClient.Failure.server("Couldn’t reach Allternit.") }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let body {
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let token: String?
        if let clerk = try await AuthManager.shared.getToken() {
            token = clerk
        } else {
            token = try await AuthManager.shared.effectiveToken()
        }
        guard let token, !token.isEmpty else { throw ACIComputersClient.Failure.signedOut }
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let (data, response) = try await URLSession.shared.data(for: request)
        return ((response as? HTTPURLResponse)?.statusCode ?? 0, data)
    }

    @MainActor
    static func checkLane() async -> Lane {
        guard let runtimeId = await RuntimeRelayStore.shared.resolveActiveRuntimeId() else {
            let devices = RuntimeRelayStore.shared.devices
            if let first = devices.first { return .offline(first.displayName) }
            return RuntimeRelayStore.shared.lastError == nil ? .none : .error("Couldn’t check your computers. Try again.")
        }
        let name = RuntimeRelayStore.shared.devices.first(where: { $0.id == runtimeId })?.displayName ?? "Your computer"
        do {
            let (status, _) = try await call("api/v1/app-phones/pair/pending?runtimeDeviceId=\(PhoneLiveSession.component(runtimeId))")
            if status == 404 || status == 405 { return .old(name) }
            if status == 401 || status == 403 { return .signedOut }
            return (200...299).contains(status) ? .ready(runtimeId: runtimeId, runtimeName: name) : .error("Couldn’t check your computers. Try again.")
        } catch let failure as ACIComputersClient.Failure {
            if case .signedOut = failure { return .signedOut }
            return .error("Couldn’t check your computers. Try again.")
        } catch {
            return .error("Couldn’t check your computers. Try again.")
        }
    }

    /// `null` = the computer has no Android lane.
    static func pending(runtimeId: String) async throws -> [Pending]? {
        let (status, data) = try await call("api/v1/app-phones/pair/pending?runtimeDeviceId=\(PhoneLiveSession.component(runtimeId))")
        if status == 404 || status == 405 { return nil }
        guard (200...299).contains(status) else { throw ACIComputersClient.Failure.server("pending \(status)") }
        return readPending(data)
    }

    static func newClaim(runtimeId: String) async throws -> Claim {
        let phoneId = UUID().uuidString.lowercased()
        let (status, data) = try await call("api/v1/app-phones/\(phoneId)/claim", method: "POST", body: ["runtimeDeviceId": runtimeId])
        if status == 429 { throw ACIComputersClient.Failure.server("Too many codes. Wait a few minutes, then try again.") }
        let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
        guard (200...299).contains(status), let code = json["code"] as? String, let id = json["phoneId"] as? String else {
            throw ACIComputersClient.Failure.server((json["message"] as? String) ?? "Couldn’t make a code. Try again.")
        }
        let seconds = (json["expiresInSec"] as? NSNumber)?.doubleValue ?? 120
        return Claim(phoneId: id, code: code, expiresAt: Date().addingTimeInterval(seconds))
    }

    /// nil = answered; otherwise the message to show.
    static func decide(phoneId: String, approve: Bool) async -> String? {
        do {
            let (status, data) = try await call("api/v1/app-phones/\(PhoneLiveSession.component(phoneId))/pair/decision", method: "POST", body: ["approve": approve])
            if (200...299).contains(status) { return nil }
            if status == 404 { return "That request expired. Ask the phone to try again." }
            let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            return (json?["message"] as? String) ?? "Your answer didn’t reach your computer. Try again."
        } catch {
            return "Your answer didn’t reach your computer. Try again."
        }
    }
}

struct AndroidScreenPairingView: View {
    @State private var lane: AndroidPairing.Lane = .checking
    @State private var claim: AndroidPairing.Claim?
    @State private var claimBusy = false
    @State private var claimError: String?
    @State private var pending: [AndroidPairing.Pending] = []
    @State private var answering: String?
    @State private var answerError: String?
    @State private var approvedName: String?
    @State private var now = Date()
    @State private var copied = false

    private var expired: Bool { claim.map { $0.expiresAt <= now } ?? false }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                Text("Android phone (screen share)")
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .accessibilityAddTraits(.isHeader)
                Text("See and tap your Android from any device.")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                content
            }
            .padding(20)
        }
        .background(Color("BgPanel").ignoresSafeArea())
        .navigationTitle("Android phone")
        .navigationBarTitleDisplayMode(.inline)
        .task { await check() }
        .task(id: laneRuntimeId) { await pollPending() }
        .onReceive(Timer.publish(every: 1, on: .main, in: .common).autoconnect()) { now = $0 }
    }

    private var laneRuntimeId: String? {
        if case .ready(let id, _) = lane { return id }
        return nil
    }

    @ViewBuilder private var content: some View {
        switch lane {
        case .checking:
            HStack(spacing: 8) { ProgressView(); Text("Checking your computer…").font(.system(size: 14)).foregroundColor(Color("TextSecondary")) }
        case .ready(let runtimeId, let runtimeName):
            steps(runtimeId: runtimeId, runtimeName: runtimeName)
        default:
            VStack(alignment: .leading, spacing: 10) {
                Text(laneText).font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                if lane != .signedOut { ACIPillButton(title: "Check again") { Task { await check() } } }
            }
        }
    }

    private var laneText: String {
        switch lane {
        case .none: return AndroidPairing.noRuntime
        case .offline(let name): return "\(name) is offline. Open Allternit Desktop on it, or start your cloud computer, then check again."
        case .old: return AndroidPairing.oldRuntime
        case .signedOut: return "Sign in to Allternit to add a phone."
        case .error(let message): return message
        default: return ""
        }
    }

    private func steps(runtimeId: String, runtimeName: String) -> some View {
        VStack(alignment: .leading, spacing: 20) {
            step(1, "Install Allternit on the phone") {
                Text("Get the Allternit app from Google Play on the Android phone.").bodyText()
                Link("Open Google Play", destination: AndroidPairing.playURL).font(.system(size: 14, weight: .semibold))
            }
            step(2, "Turn on taps") {
                Text("On the phone, open Settings › Accessibility › Installed apps › Allternit and turn it on. Without it you can watch the screen but not tap.").bodyText()
                Text("Android 13 or newer, with Allternit installed from a file instead of Google Play: if the switch is greyed out, open Settings › Apps › Allternit, tap ⋮ › Allow restricted settings, then turn it on.").bodyText()
            }
            step(3, "Link the phone") {
                Text("In the Allternit app, tap Share screen › Link with a code and type this code. Or scan the code with the phone’s camera.").bodyText()
                if let claim, !expired { claimCard(claim) } else {
                    ACIPillButton(title: claimBusy ? "Making a code…" : expired ? "Get a new code" : "Get a code") {
                        Task { await newCode(runtimeId) }
                    }
                    .disabled(claimBusy)
                }
                if let claimError { Text(claimError).font(.system(size: 13)).foregroundColor(Theme.statusError) }
            }
            step(4, "Approve the phone") {
                if let approvedName {
                    Text("\(approvedName) is linked. Open it from Settings › Computers › Phones, then tap Start on the phone.").bodyText()
                } else if pending.isEmpty {
                    HStack(spacing: 8) { ProgressView(); Text("Waiting for your phone…").bodyText() }
                } else {
                    ForEach(pending) { pair in pendingRow(pair) }
                }
                if let answerError { Text(answerError).font(.system(size: 13)).foregroundColor(Theme.statusError) }
            }
            Text("The screen goes through \(runtimeName). When you open the phone here, tap Start on the phone to share.").bodyText()
        }
    }

    private func step<Body: View>(_ n: Int, _ title: String, @ViewBuilder _ body: () -> Body) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Text("\(n)")
                .font(.system(size: 12, weight: .semibold))
                .frame(width: 24, height: 24)
                .background(Color("FillNeutral"))
                .clipShape(Circle())
                .foregroundColor(Color("TextPrimary"))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 8) {
                Text(title).font(.system(size: 15, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                body()
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func claimCard(_ claim: AndroidPairing.Claim) -> some View {
        let left = max(0, Int(claim.expiresAt.timeIntervalSince(now).rounded()))
        return VStack(alignment: .leading, spacing: 10) {
            if let link = AndroidPairing.deepLink(claim.code), let image = Self.qr(link.absoluteString) {
                Image(uiImage: image)
                    .interpolation(.none)
                    .resizable()
                    .frame(width: 148, height: 148)
                    .padding(8)
                    .background(Color.white)
                    .clipShape(RoundedRectangle(cornerRadius: 10))
                    .accessibilityLabel("Code to scan with your Android phone")
            }
            Text(claim.code)
                .font(.system(size: 26, weight: .semibold, design: .monospaced))
                .foregroundColor(Color("TextPrimary"))
                .accessibilityLabel("Code \(claim.code.map(String.init).joined(separator: " "))")
            Text("Works once, for \(left / 60):\(String(format: "%02d", left % 60)) more").bodyText()
            HStack(spacing: 8) {
                ACIPillButton(title: copied ? "Copied" : "Copy") {
                    UIPasteboard.general.string = claim.code
                    copied = true
                }
                if let link = AndroidPairing.deepLink(claim.code) {
                    ShareLink(item: link) {
                        Text("Share")
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .padding(.horizontal, 18)
                            .frame(minWidth: 64, minHeight: 44)
                            .background(Color("FillNeutral"))
                            .clipShape(Capsule())
                    }
                }
            }
        }
    }

    private func pendingRow(_ pair: AndroidPairing.Pending) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(pair.model.isEmpty || pair.model == pair.name ? pair.name : "\(pair.name) · \(pair.model)")
                .font(.system(size: 15, weight: .medium)).foregroundColor(Color("TextPrimary"))
            Text("Approve only if the phone shows \(pair.code)").bodyText()
            HStack(spacing: 8) {
                ACIPillButton(title: "Deny") { Task { await answer(pair, approve: false) } }
                    .disabled(answering != nil)
                ACIPillButton(title: answering == pair.phoneId ? "Approving…" : "Approve") { Task { await answer(pair, approve: true) } }
                    .disabled(answering != nil)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgSecondary"))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }

    // MARK: Actions

    private func check() async {
        lane = .checking
        await RuntimeRelayStore.shared.refresh()
        lane = await AndroidPairing.checkLane()
    }

    private func pollPending() async {
        guard let runtimeId = laneRuntimeId else { return }
        while !Task.isCancelled {
            if let list = try? await AndroidPairing.pending(runtimeId: runtimeId) {
                pending = list
            }
            try? await Task.sleep(for: .seconds(3))
        }
    }

    private func newCode(_ runtimeId: String) async {
        claimBusy = true
        claimError = nil
        copied = false
        defer { claimBusy = false }
        do {
            claim = try await AndroidPairing.newClaim(runtimeId: runtimeId)
            now = Date()
        } catch {
            claimError = error.localizedDescription.isEmpty ? "Couldn’t make a code. Try again." : error.localizedDescription
        }
    }

    private func answer(_ pair: AndroidPairing.Pending, approve: Bool) async {
        answering = pair.phoneId
        answerError = nil
        let message = await AndroidPairing.decide(phoneId: pair.phoneId, approve: approve)
        answering = nil
        if let message {
            answerError = message
            return
        }
        pending.removeAll { $0.phoneId == pair.phoneId }
        if approve {
            approvedName = pair.name
            await ConnectionsStore.shared.load()
        }
    }

    static func qr(_ text: String) -> UIImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(text.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 8, y: 8)),
              let cg = CIContext().createCGImage(output, from: output.extent) else { return nil }
        return UIImage(cgImage: cg)
    }
}

private extension Text {
    func bodyText() -> some View {
        self.font(.system(size: 13)).foregroundColor(Color("TextSecondary")).fixedSize(horizontal: false, vertical: true)
    }
}
