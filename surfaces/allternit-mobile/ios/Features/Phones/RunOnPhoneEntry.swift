import SwiftUI

/// "Run on my phone" in the composer "+" sheet, Option A (Eoj, 2026-11-10):
/// a row that, when a phone is ready, arms the "On <phone>" chip and closes
/// the sheet. Otherwise it says why: no phone linked, phone offline, signed
/// out, quota reached.
enum RunOnPhoneState: Equatable {
    case idle, checking
    case ready([ACIComputersClient.CloudPhone])
    case noPhone, offline(String), signedOut, quota
    case failed(String)
}

struct RunOnPhoneRow: View {
    /// Debug gallery: show one state without loading.
    var fixedState: RunOnPhoneState? = nil
    var onArmed: () -> Void = {}

    @ObservedObject private var store = PhoneRunStore.shared
    @State private var state: RunOnPhoneState = .idle

    private var current: RunOnPhoneState { fixedState ?? state }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Button { Task { await check() } } label: {
                HStack(spacing: 12) {
                    Image(systemName: "iphone")
                        .font(.system(size: 15))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 32, height: 32)
                        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 8))
                    VStack(alignment: .leading, spacing: 1) {
                        Text("Run on my phone")
                            .font(.system(size: 14, weight: .medium))
                            .foregroundColor(Color("TextPrimary"))
                        Text(subtitle)
                            .font(.system(size: 11))
                            .foregroundColor(Color("TextSecondary"))
                    }
                    Spacer(minLength: 8)
                    if current == .checking {
                        ProgressView().controlSize(.small)
                    } else {
                        Image(systemName: "chevron.right")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundColor(Color("TextSecondary"))
                            .opacity(0.6)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityHint("Your next message runs as a task on your linked phone")

            notice
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("FillNeutral").opacity(0.6), in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private var subtitle: String {
        if let armed = store.armed, fixedState == nil { return "On \(armed.name). Your next message runs there." }
        return "Your bot does the task on your linked phone"
    }

    @ViewBuilder
    private var notice: some View {
        switch current {
        case .idle, .checking:
            EmptyView()
        case .ready(let phones):
            // More than one live phone: pick which one the chip names.
            VStack(spacing: 6) {
                ForEach(phones) { phone in
                    Button { arm(phone) } label: {
                        HStack {
                            Text(phone.name).font(.system(size: 13, weight: .medium))
                            Spacer()
                            Text("Ready").font(.system(size: 11)).foregroundColor(.green)
                        }
                        .foregroundColor(Color("TextPrimary"))
                        .padding(10)
                        .background(Color("BgPrimary"), in: RoundedRectangle(cornerRadius: 10))
                    }
                    .buttonStyle(.plain)
                }
            }
        case .noPhone:
            message("No phone linked yet", "No phone is linked, so your bot has nothing to use.", action: "Open Phone Link") {
                DeepLinkRouter.shared.enqueue(.computers)
            }
        case .offline(let name):
            message("Phone offline", "\(name) is not reachable. Open Allternit Desktop and check the phone.", action: "Open Phone Link") {
                DeepLinkRouter.shared.enqueue(.computers)
            }
        case .signedOut:
            message("Sign in to use your phone", "Sign in to run tasks on your phone.", action: nil) {}
        case .quota:
            message("Quota reached", "Phone minutes used up this month.", action: "See usage") {
                DeepLinkRouter.shared.enqueue(.settings("billing"))
            }
        case .failed(let why):
            message("Couldn't check your phone", why, action: "Try again") { Task { await check() } }
        }
    }

    private func message(_ title: String, _ body: String, action: String?, _ run: @escaping () -> Void) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            Text(body).font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
            if let action {
                Button(action, action: run)
                    .font(.system(size: 12, weight: .semibold))
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"), in: RoundedRectangle(cornerRadius: 10))
    }

    private func arm(_ phone: ACIComputersClient.CloudPhone) {
        store.arm(phone)
        onArmed()
    }

    private func check() async {
        guard fixedState == nil, state != .checking else { return }
        guard AuthManager.shared.isSignedIn else { state = .signedOut; return }
        state = .checking
        do {
            let phones = try await PhoneTaskClient.shared.phones()
            let live = phones.filter(\.isLive)
            if phones.isEmpty { state = .noPhone }
            else if live.isEmpty { state = .offline(phones[0].name) }
            else if live.count == 1 { state = .idle; arm(live[0]) }
            else { state = .ready(live) }
        } catch PhoneTaskClient.Failure.signedOut {
            state = .signedOut
        } catch PhoneTaskClient.Failure.quota {
            state = .quota
        } catch {
            state = .failed(error.localizedDescription)
        }
    }
}

/// The "On <phone>" chip above the composer text while a phone is armed.
struct PhoneArmedChip: View {
    let name: String
    let onClear: () -> Void

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "iphone").font(.system(size: 11, weight: .semibold))
            Text("On \(name)").font(.system(size: 12, weight: .semibold)).lineLimit(1)
            Button(action: onClear) {
                Image(systemName: "xmark").font(.system(size: 9, weight: .bold))
                    .frame(width: 18, height: 18)
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Stop running on \(name)")
        }
        .foregroundColor(Color("TextPrimary"))
        .padding(.leading, 10)
        .padding(.trailing, 4)
        .padding(.vertical, 4)
        .background(Color("FillNeutral"), in: Capsule())
        .overlay(Capsule().stroke(Color("BorderSubtle")))
        .accessibilityElement(children: .combine)
    }
}
