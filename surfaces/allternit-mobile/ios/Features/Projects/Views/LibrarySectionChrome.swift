import SwiftUI
import UIKit

/// The phone section chrome the drawer's list screens share (allternit-ai
/// src/fabric-session/phone/PhoneSectionChrome.tsx + phone.css): a top bar
/// with a round ☰, a centered title and a round filter button, and a
/// floating bottom bar with search and New. The list scrolls under both.
/// Projects, Artifacts and Automation Tasks use it; each screen keeps its own
/// data, filters and menus.

typealias LibraryLayout = OverviewLayout

/// Opens the shell drawer with the shell's spring.
@MainActor
func openLibraryDrawer(_ isSidebarOpen: Binding<Bool>) {
    UIImpactFeedbackGenerator(style: .medium).impactOccurred()
    withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
        isSidebarOpen.wrappedValue = true
    }
}

/// A filled 44pt circle (`.phone-round-btn` in the section head).
struct LibraryRoundButton: View {
    let icon: String
    let label: String
    var dot = false
    let action: () -> Void

    var body: some View {
        Button(action: action) { LibraryRoundLabel(icon: icon, dot: dot) }
            .buttonStyle(.plain)
            .accessibilityLabel(label)
    }
}

/// The round button's face, for use as a `Menu` label.
struct LibraryRoundLabel: View {
    let icon: String
    var dot = false

    var body: some View {
        Image(systemName: icon)
            .font(.system(size: 18, weight: .regular))
            .foregroundColor(Color("TextPrimary"))
            .frame(width: 44, height: 44)
            .background(Circle().fill(Color("FillNeutral")))
            .overlay(alignment: .topTrailing) {
                if dot {
                    Circle()
                        .fill(Color("TextPrimary"))
                        .frame(width: 7, height: 7)
                        .overlay(Circle().stroke(Color("BgPrimary"), lineWidth: 2))
                        .offset(x: -9, y: 9)
                }
            }
            .contentShape(Circle())
    }
}

/// ☰ (round) · centered title · filter slot (round). 52pt tall, 12pt sides.
struct LibrarySectionHeader<Filter: View>: View {
    var title: String?
    let onMenu: () -> Void
    @ViewBuilder var filter: () -> Filter

    var body: some View {
        ZStack {
            if let title {
                Text(title)
                    .font(.system(size: 17, weight: .semibold))
                    .tracking(-0.2)
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                    .padding(.horizontal, 56)
            }
            HStack {
                LibraryRoundButton(icon: "line.3.horizontal", label: "Open menu", action: onMenu)
                Spacer()
                filter()
            }
        }
        .frame(height: 52)
        .padding(.horizontal, 12)
        .background(Color("BgPrimary"))
    }
}

/// A plain ☰ bar (PhoneHeader with no title): Customize and an open project.
struct LibraryPlainHeader: View {
    let onMenu: () -> Void

    var body: some View {
        HStack {
            Button(action: onMenu) {
                Image(systemName: "line.3.horizontal")
                    .font(.system(size: 20, weight: .regular))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Open menu")
            Spacer()
        }
        .frame(height: 52)
        .padding(.horizontal, 6)
        .background(Color("BgPrimary"))
    }
}

/// Floating bottom bar: search + New (`.phone-float-bar`).
/// `.search`: a search pill fills the row and New is a round +.
/// `.new`: a round search button and a "New …" pill; tapping search opens
/// the pill (and New shrinks to +) until the search is cleared.
struct LibraryFloatingBar<NewMenu: View>: View {
    enum Wide { case search, new }

    @Binding var query: String
    var placeholder = "Search"
    let newLabel: String
    var wide: Wide = .search
    var newDisabled = false
    /// Plain New action; ignored when `newMenu` is non-empty.
    var onNew: (() -> Void)? = nil
    /// Menu items for New (the web's renderNew menus).
    @ViewBuilder var newMenu: () -> NewMenu

    @State private var open = false
    @FocusState private var focused: Bool

    private var searching: Bool { wide == .search || open || !query.isEmpty }

    var body: some View {
        HStack(spacing: 10) {
            if searching {
                searchPill
            } else {
                Button(action: openSearch) {
                    Image(systemName: "magnifyingglass")
                        .font(.system(size: 19))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 46, height: 46)
                        .modifier(FloatSurface(shape: Circle()))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(placeholder)
                Spacer(minLength: 0)
            }
            newButton
        }
        .padding(.horizontal, 16)
        .padding(.bottom, 14)
    }

    private var searchPill: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 17))
                .foregroundColor(Color("TextSecondary"))
            TextField(placeholder, text: $query)
                .font(.system(size: 16))
                .foregroundColor(Color("TextPrimary"))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.search)
                .focused($focused)
                .onChange(of: focused) { _, isFocused in
                    if !isFocused && wide == .new && query.isEmpty { open = false }
                }
            if !query.isEmpty || wide == .new {
                Button(action: clear) {
                    Image(systemName: "xmark")
                        .font(.system(size: 12, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 30, height: 30)
                        .background(Circle().fill(Color("BorderSubtle")))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.leading, 16)
        .padding(.trailing, 8)
        .frame(height: 46)
        .frame(maxWidth: .infinity)
        .modifier(FloatSurface(shape: Capsule()))
    }

    @ViewBuilder
    private var newButton: some View {
        let face = newFace
        if NewMenu.self != EmptyView.self {
            Menu { newMenu() } label: { face }
                .disabled(newDisabled)
                .accessibilityLabel(newLabel)
        } else {
            Button(action: { onNew?() }) { face }
                .buttonStyle(.plain)
                .disabled(newDisabled)
                .accessibilityLabel(newLabel)
        }
    }

    @ViewBuilder
    private var newFace: some View {
        if searching {
            Image(systemName: "plus")
                .font(.system(size: 20, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 46, height: 46)
                .modifier(FloatSurface(shape: Circle()))
                .opacity(newDisabled ? 0.6 : 1)
        } else {
            HStack(spacing: 8) {
                Image(systemName: "plus")
                    .font(.system(size: 17, weight: .semibold))
                Text(newLabel)
                    .font(.system(size: 16, weight: .medium))
                    .lineLimit(1)
            }
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 18)
            .frame(height: 46)
            .modifier(FloatSurface(shape: Capsule()))
            .opacity(newDisabled ? 0.6 : 1)
        }
    }

    private func openSearch() {
        open = true
        focused = true
    }

    private func clear() {
        query = ""
        if wide == .new {
            open = false
            focused = false
        } else {
            focused = true
        }
    }
}

extension LibraryFloatingBar where NewMenu == EmptyView {
    init(query: Binding<String>, placeholder: String = "Search", newLabel: String, wide: Wide = .search,
         newDisabled: Bool = false, onNew: @escaping () -> Void) {
        self._query = query
        self.placeholder = placeholder
        self.newLabel = newLabel
        self.wide = wide
        self.newDisabled = newDisabled
        self.onNew = onNew
        self.newMenu = { EmptyView() }
    }
}

/// `.phone-float-*` surface: elevated fill, hairline, soft shadow.
private struct FloatSurface<S: Shape>: ViewModifier {
    let shape: S
    @Environment(\.colorScheme) private var scheme

    func body(content: Content) -> some View {
        content
            .background(shape.fill(Color("BgPanel")))
            .overlay(shape.stroke(Color("BorderStrong"), lineWidth: 0.5))
            .shadow(color: .black.opacity(scheme == .dark ? 0.45 : 0.14), radius: 8, x: 0, y: 4)
            .contentShape(shape)
    }
}

/// A checkmarked menu row (the web's `shortcut="✓"`).
struct LibraryMenuCheck: View {
    let title: String
    var icon: String? = nil
    var hint: String? = nil
    let checked: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            if checked {
                Label(hint.map { "\(title)  \($0)" } ?? title, systemImage: "checkmark")
            } else if let icon {
                Label(hint.map { "\(title)  \($0)" } ?? title, systemImage: icon)
            } else {
                Text(hint.map { "\(title)  \($0)" } ?? title)
            }
        }
    }
}

/// "now", "5m", "3h", "2d", "Mar 4" (useModeOverview shortAgo).
func libraryShortAgo(_ date: Date?, now: Date = Date()) -> String {
    guard let date else { return "" }
    let seconds = max(0, now.timeIntervalSince(date))
    if seconds < 60 { return "now" }
    if seconds < 3600 { return "\(Int(seconds / 60))m" }
    if seconds < 86_400 { return "\(Int(seconds / 3600))h" }
    if seconds < 7 * 86_400 { return "\(Int(seconds / 86_400))d" }
    return date.formatted(.dateTime.month(.abbreviated).day())
}

/// Parses the API's ISO-8601 and "YYYY-MM-DD HH:MM:SS" timestamps.
func libraryParseDate(_ raw: String?) -> Date? {
    guard let raw, !raw.isEmpty else { return nil }
    let iso = ISO8601DateFormatter()
    iso.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let date = iso.date(from: raw) { return date }
    iso.formatOptions = [.withInternetDateTime]
    if let date = iso.date(from: raw) { return date }
    let plain = DateFormatter()
    plain.locale = Locale(identifier: "en_US_POSIX")
    plain.timeZone = TimeZone(identifier: "UTC")
    plain.dateFormat = "yyyy-MM-dd HH:mm:ss"
    return plain.date(from: raw)
}
