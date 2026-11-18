import SwiftUI

/// Projects screen chrome (phone calm step 27), mirroring the web's
/// `PhoneProjectsChrome.tsx`: a glass search + filter pill, a floating
/// "+ New project" button and the one-time explainer.
enum ProjectsExplainer {
    /// Same text as web `PROJECTS_EXPLAINER_TITLE` / `PROJECTS_EXPLAINER_BODY`.
    static let title = "Every project is a folder your team works in"
    static let body = "Put chats, code, designs and bots in one project. Everyone on it shares the same files, context and instructions."
    /// Web once-key is `allternit-phone:nudge:projects-explainer`.
    static let onceKey = "allternit.nudge.projects-explainer"

    static func seen(_ defaults: UserDefaults = .standard) -> Bool {
        defaults.bool(forKey: onceKey)
    }

    static func markSeen(_ defaults: UserDefaults = .standard) {
        defaults.set(true, forKey: onceKey)
    }
}

/// Glass pill, top-right: search and the filter popover (which holds the
/// List · Grid · Deck switch and the existing filters).
struct ProjectsPill<FilterContent: View>: View {
    @Binding var searching: Bool
    var filtersActive: Bool
    @ViewBuilder var filterContent: () -> FilterContent

    @State private var filterOpen = false

    var body: some View {
        HStack(spacing: 0) {
            Button {
                Haptics.light()
                searching.toggle()
            } label: {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 16, weight: searching ? .semibold : .regular))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Search projects")
            .accessibilityAddTraits(searching ? .isSelected : [])

            Button {
                Haptics.light()
                filterOpen = true
            } label: {
                Image(systemName: "slider.horizontal.3")
                    .font(.system(size: 16, weight: filtersActive ? .semibold : .regular))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 44, height: 44)
                    .overlay(alignment: .topTrailing) {
                        if filtersActive { PhoneRoundDot().offset(x: -9, y: 9) }
                    }
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Filter and layout")
            .popover(isPresented: $filterOpen, arrowEdge: .top) {
                filterContent()
                    .presentationCompactAdaptation(.popover)
            }
        }
        .padding(.horizontal, 2)
        .glassSurface(cornerRadius: 22)
    }
}

/// Inline glass search field shown under the header while searching.
struct ProjectsSearchField: View {
    @Binding var query: String
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 14))
                .foregroundColor(Color("TextTertiary"))
            TextField("Search projects", text: $query)
                .font(.system(size: 15))
                .focused($focused)
                .submitLabel(.search)
            if !query.isEmpty {
                Button { query = "" } label: {
                    Image(systemName: "xmark.circle.fill").foregroundColor(Color("TextTertiary"))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 14)
        .frame(height: 44)
        .glassSurface(cornerRadius: 22)
        .padding(.horizontal, 16)
        .padding(.bottom, 6)
        .onAppear { focused = true }
    }
}

/// Floating glass "+ New project".
struct ProjectsNewButton: View {
    var busy = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: "plus").font(.system(size: 15, weight: .semibold))
                Text(busy ? "Creating…" : "New project").font(.system(size: 16, weight: .semibold))
            }
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 22)
            .frame(height: 50)
            .glassSurface(cornerRadius: 25)
        }
        .buttonStyle(.plain)
        .disabled(busy)
        .accessibilityLabel("New project")
    }
}

/// One-time explainer card; OK opens the create flow.
struct ProjectsExplainerCard: View {
    let onOK: () -> Void
    let onDismiss: () -> Void
    @ReducedMotion private var reduced

    var body: some View {
        ZStack {
            Color.black.opacity(0.28)
                .ignoresSafeArea()
                .onTapGesture(perform: onDismiss)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 12) {
                Text(ProjectsExplainer.title)
                    .font(.system(.title3, design: .serif).weight(.medium))
                    .foregroundColor(Color("TextPrimary"))
                    .accessibilityAddTraits(.isHeader)
                Text(ProjectsExplainer.body)
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextSecondary"))
                Button(action: onOK) {
                    Text("OK")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Color("BgPrimary"))
                        .frame(maxWidth: .infinity)
                        .frame(height: 48)
                        .background(Capsule().fill(Color("TextPrimary")))
                }
                .buttonStyle(.plain)
                .padding(.top, 4)
            }
            .padding(20)
            .background(RoundedRectangle(cornerRadius: 24, style: .continuous).fill(Color("BgPrimary")))
            .padding(.horizontal, 28)
            .transition(reduced ? .opacity : .opacity.combined(with: .scale(scale: 0.96)))
            .accessibilityElement(children: .contain)
            .accessibilityAddTraits(.isModal)
        }
        .onAppear { Haptics.light() }
    }
}
