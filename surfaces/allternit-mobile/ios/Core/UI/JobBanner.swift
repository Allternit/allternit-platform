import SwiftUI
import UIKit

/// A quiet capsule above the composer for long jobs: "Working on <task>".
/// One at a time; the text cross-fades when a job updates, and the banner
/// leaves a moment after it is done.
@MainActor
final class JobBannerCenter: ObservableObject {
    static let shared = JobBannerCenter()

    struct Banner: Equatable {
        let id: String
        var kind: String
        var title: String
        var done: Bool
    }

    @Published private(set) var current: Banner?

    func show(id: String, kind: String, title: String) {
        current = Banner(id: id, kind: kind, title: title, done: false)
        announce(title)
    }

    func update(id: String, title: String, done: Bool) {
        guard var banner = current, banner.id == id else { return }
        banner.title = title
        banner.done = done
        current = banner
        announce(title)
        if done {
            Task { [weak self] in
                try? await Task.sleep(for: .seconds(2.5))
                self?.dismiss(id: id)
            }
        }
    }

    func dismiss(id: String) {
        if current?.id == id { current = nil }
    }

    private func announce(_ text: String) {
        UIAccessibility.post(notification: .announcement, argument: text)
    }

    static func symbol(for kind: String) -> String {
        switch kind {
        case "compact": return "arrow.down.right.and.arrow.up.left"
        case "image": return "photo"
        case "doc": return "doc.text"
        case "search": return "magnifyingglass"
        default: return "gearshape"
        }
    }
}

struct JobBannerLayer: View {
    @ObservedObject private var center = JobBannerCenter.shared
    @ReducedMotion private var reduceMotion

    var body: some View {
        VStack {
            Spacer(minLength: 0)
            if let banner = center.current {
                HStack(spacing: 8) {
                    Image(systemName: banner.done ? "checkmark" : JobBannerCenter.symbol(for: banner.kind))
                        .font(.caption.weight(.semibold))
                    Text(banner.title)
                        .font(.footnote.weight(.medium))
                        .lineLimit(1)
                        .id(banner.title)
                        .transition(.opacity)
                }
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
                .background(.regularMaterial, in: Capsule())
                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                .padding(.bottom, 96)
                .transition(reduceMotion ? .opacity : .move(edge: .bottom).combined(with: .opacity))
                .accessibilityElement(children: .combine)
            }
        }
        .animation(reduceMotion ? .linear(duration: 0.12) : .spring(response: 0.38, dampingFraction: 1), value: center.current)
        .allowsHitTesting(false)
    }
}
