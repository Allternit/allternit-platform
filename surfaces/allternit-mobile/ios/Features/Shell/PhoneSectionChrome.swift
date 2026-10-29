import SwiftUI
import UIKit

// Phone chrome for list screens (allternit-ai src/fabric-session/phone/
// PhoneSectionChrome.tsx), after the Claude iOS pattern: a top bar with a
// round ☰, a centered title and a round filter button, and a floating
// bottom bar with a search pill and a New button. The list scrolls under
// both. Screens keep their own data, filters and menus; these only place them.

/// ☰ (round) · centered 17pt title · filter slot (round).
struct PhoneSectionHeader<Filter: View>: View {
    var title: String?
    let onMenu: () -> Void
    @ViewBuilder var filter: () -> Filter

    var body: some View {
        PhoneHeader(title: title, sectionStyle: true, onMenu: onMenu, trailing: filter)
    }
}

extension PhoneSectionHeader where Filter == EmptyView {
    init(title: String?, onMenu: @escaping () -> Void) {
        self.init(title: title, onMenu: onMenu) { EmptyView() }
    }
}

/// Round filter/sort button's face; a dot shows when a filter is on. Use it
/// as a `Menu` label. Screens with List · Grid · Deck pass their layout's icon.
struct PhoneFilterButtonLabel: View {
    var active: Bool = false
    var systemName: String = "slider.horizontal.3"

    var body: some View {
        Image(systemName: systemName)
            .font(.system(size: 18, weight: .regular))
            .foregroundColor(Color("TextPrimary"))
            .frame(width: 44, height: 44)
            .background(Circle().fill(Color("FillNeutral")))
            .overlay(alignment: .topTrailing) {
                if active { PhoneRoundDot().offset(x: -9, y: 9) }
            }
            .contentShape(Circle())
    }
}

/// Floating bottom bar: search + New.
/// `.search`: a search pill fills the row and New is a round +.
/// `.new`: a round search button and a "New …" pill; tapping search opens
/// the pill (and New shrinks to +) until the search is cleared.
struct PhoneFloatingBar: View {
    enum Wide { case search, new }

    @Binding var query: String
    var placeholder: String = "Search"
    /// "New project", "New task", … (also the round button's label).
    let newLabel: String
    var wide: Wide = .search
    var newDisabled: Bool = false
    let onNew: () -> Void

    @State private var open = false
    @FocusState private var focused: Bool

    private var searching: Bool { wide == .search || open || !query.isEmpty }

    var body: some View {
        HStack(spacing: 10) {
            if searching {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass")
                        .font(.system(size: 16))
                        .foregroundColor(Color("TextSecondary"))
                    TextField(placeholder, text: $query)
                        .font(.system(size: 16))
                        .foregroundColor(Color("TextPrimary"))
                        .submitLabel(.search)
                        .focused($focused)
                        .onChange(of: focused) { _, isFocused in
                            if !isFocused, wide == .new, query.isEmpty { open = false }
                        }
                    if !query.isEmpty || wide == .new {
                        Button {
                            query = ""
                            if wide == .new { open = false; focused = false } else { focused = true }
                        } label: {
                            Image(systemName: "xmark")
                                .font(.system(size: 12, weight: .bold))
                                .foregroundColor(Color("TextSecondary"))
                                .frame(width: 28, height: 28)
                                .contentShape(Circle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Clear search")
                    }
                }
                .padding(.leading, 16)
                .padding(.trailing, 8)
                .frame(height: 46)
                .frame(maxWidth: .infinity)
                .background(Capsule().fill(Color("BgPanel")))
                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 0.5))
                .shadow(color: .black.opacity(0.14), radius: 8, x: 0, y: 4)
            } else {
                Button {
                    open = true
                    focused = true
                } label: {
                    floatingRound("magnifyingglass")
                }
                .buttonStyle(.plain)
                .accessibilityLabel(placeholder)
                Spacer(minLength: 0)
            }
            Button {
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                onNew()
            } label: {
                if searching {
                    floatingRound("plus", weight: .bold)
                } else {
                    HStack(spacing: 8) {
                        Image(systemName: "plus").font(.system(size: 16, weight: .bold))
                        Text(newLabel).font(.system(size: 16, weight: .medium))
                    }
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 18)
                    .frame(height: 46)
                    .background(Capsule().fill(Color("BgPanel")))
                    .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 0.5))
                    .shadow(color: .black.opacity(0.14), radius: 8, x: 0, y: 4)
                }
            }
            .buttonStyle(.plain)
            .disabled(newDisabled)
            .opacity(newDisabled ? 0.5 : 1)
            .accessibilityLabel(newLabel)
            .accessibilityIdentifier("phoneSectionNew")
        }
        .padding(.horizontal, 16)
        .padding(.bottom, 14)
        .animation(.spring(response: 0.3, dampingFraction: 0.86), value: searching)
    }

    private func floatingRound(_ systemName: String, weight: Font.Weight = .regular) -> some View {
        Image(systemName: systemName)
            .font(.system(size: 18, weight: weight))
            .foregroundColor(Color("TextPrimary"))
            .frame(width: 46, height: 46)
            .background(Circle().fill(Color("BgPanel")))
            .overlay(Circle().stroke(Color("BorderSubtle"), lineWidth: 0.5))
            .shadow(color: .black.opacity(0.14), radius: 8, x: 0, y: 4)
            .contentShape(Circle())
    }
}
