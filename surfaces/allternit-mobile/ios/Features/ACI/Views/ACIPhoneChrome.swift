import SwiftUI

// Shared pieces of the web phone's ACI screen (allternit-ai
// src/fabric-session/phone/screens/AciScreen.tsx + phone.css): the ☰-only
// header, the desktop page header (ViewHeader), the one-row compact bar
// (CompactViewBar), the promo hero and the pair / teach nudge cards.

// MARK: - Header

/// PhoneHeader on ACI: each view names itself below, so the bar keeps only ☰.
struct ACIPhoneHeader: View {
    var onMenu: () -> Void

    var body: some View {
        HStack(spacing: 0) {
            Button {
                UIImpactFeedbackGenerator(style: .medium).impactOccurred()
                onMenu()
            } label: {
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
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(Color("BgPrimary"))
    }
}

/// ViewHeader (components/ui/polish/ViewHeader.tsx): 30 pt serif title,
/// optional 14 pt subtitle. The phone sets `--view-header-top` to 8.
struct ACIViewHeader: View {
    let title: String
    var subtitle: String? = nil
    /// `!pb-4` on the compact views, the default `pb-6` elsewhere.
    var bottom: CGFloat = 24

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.system(size: 30, weight: .medium, design: .serif))
                .tracking(-0.3)
                .foregroundColor(Color("TextPrimary"))
                .accessibilityAddTraits(.isHeader)
            if let subtitle {
                Text(subtitle)
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, 8)
        .padding(.bottom, bottom)
    }
}

// MARK: - Compact bar

/// CompactViewBar: filters on the left (scrolling sideways), a search button
/// that opens into a field, then icon actions. 48 pt tall, hairline below.
struct ACICompactBar<Filters: View, Actions: View>: View {
    @Binding var query: String
    let searchLabel: String
    let placeholder: String
    let filters: () -> Filters
    let actions: () -> Actions

    @State private var searching = false
    @FocusState private var focused: Bool

    init(query: Binding<String>, searchLabel: String, placeholder: String,
         @ViewBuilder filters: @escaping () -> Filters,
         @ViewBuilder actions: @escaping () -> Actions) {
        self._query = query
        self.searchLabel = searchLabel
        self.placeholder = placeholder
        self.filters = filters
        self.actions = actions
    }

    var body: some View {
        HStack(spacing: 4) {
            if searching {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextTertiary"))
                    TextField(placeholder, text: $query)
                        .font(.system(size: 15))
                        .foregroundColor(Color("TextPrimary"))
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.search)
                        .focused($focused)
                        .accessibilityLabel(searchLabel)
                }
                .padding(.horizontal, 12)
                .frame(height: 36)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .overlay(
                    RoundedRectangle(cornerRadius: 8)
                        .stroke(focused ? Color("TextPrimary") : Color("BorderStrong"), lineWidth: 1)
                )
                ACIBarIcon(systemName: "xmark", label: "Close search") {
                    query = ""
                    searching = false
                }
            } else {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 4) { filters() }
                }
                ACIBarIcon(systemName: "magnifyingglass", label: searchLabel) {
                    searching = true
                }
                actions()
            }
        }
        .padding(.horizontal, 12)
        .frame(height: 48)
        .overlay(alignment: .bottom) {
            Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
        }
        .onChange(of: searching) { _, now in
            if now { focused = true }
        }
        .onAppear { if !query.isEmpty { searching = true } }
    }
}

/// COMPACT_BAR_ICON: a 36 pt icon button in the compact bar.
struct ACIBarIcon: View {
    let systemName: String
    let label: String
    var isOn: Bool = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemName)
                .font(.system(size: 17))
                .foregroundColor(isOn ? Color("TextPrimary") : Color("TextSecondary"))
                .frame(width: 36, height: 36)
                .background(isOn ? Color("FillNeutral") : Color.clear)
                .clipShape(RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

/// A filter or sub-tab chip in the compact bar: filled near-black when on.
struct ACIBarChip: View {
    let title: String
    let isOn: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(isOn ? Color("OnAccent") : Color("TextSecondary"))
                .padding(.horizontal, 12)
                .frame(height: 32)
                .background(isOn ? Color("AccentPrimary") : Color.clear)
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .fixedSize()
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

// MARK: - Buttons

/// `.phone-add-computer`: a full-width 44 pt filled button with a hairline.
struct ACIWideButton: View {
    let systemName: String
    let title: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: systemName)
                    .font(.system(size: 18))
                Text(title)
                    .font(.system(size: 16, weight: .semibold))
            }
            .foregroundColor(Color("TextPrimary"))
            .frame(maxWidth: .infinity, minHeight: 44)
            .background(Color("FillNeutral"))
            .clipShape(RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .padding(.bottom, 12)
    }
}

/// `.phone-phone-view-btn` / `.phone-phones-retry`: a 44 pt capsule.
struct ACIPillButton: View {
    let title: String
    var accessibilityLabel: String? = nil
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 18)
                .frame(minWidth: 64, minHeight: 44)
                .background(Color("FillNeutral"))
                .clipShape(Capsule())
                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(accessibilityLabel ?? title)
    }
}

/// `.phone-primary-btn` in a promo sheet: 52 pt, fully rounded, near-black.
struct ACIPrimaryButton: View {
    let title: String
    var height: CGFloat = 52
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(Color("OnAccent"))
                .frame(maxWidth: .infinity, minHeight: height)
                .background(Color("AccentPrimary"))
                .clipShape(Capsule())
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Promo hero + nudge cards

/// PromoHero.tsx: a brand mark over a grid illustration with two tilted
/// cards peeking from the bottom edge.
struct ACIPromoHero: View {
    enum Kind { case pair, setup }

    let kind: Kind
    var height: CGFloat = 132
    var imageHeight: CGFloat = 64
    var topRadius: CGFloat = 16

    var body: some View {
        ZStack {
            Color("FillNeutral")
            gridLines
            // The two cards: 84×56, 18 pt in from each side, 18 pt below the edge.
            GeometryReader { proxy in
                card.rotationEffect(.degrees(-7))
                    .position(x: 18 + 42, y: proxy.size.height + 18 - 28)
                card.rotationEffect(.degrees(6))
                    .position(x: proxy.size.width - 18 - 42, y: proxy.size.height + 18 - 28)
            }
            Image(kind == .pair ? "MatrixLogo" : "GizziMascot")
                .resizable()
                .scaledToFit()
                .frame(height: imageHeight)
        }
        .frame(height: height)
        .clipShape(UnevenRoundedRectangle(topLeadingRadius: topRadius, topTrailingRadius: topRadius))
        .accessibilityHidden(true)
    }

    private var card: some View {
        RoundedRectangle(cornerRadius: 12)
            .fill(Color("BgPrimary"))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderStrong"), lineWidth: 1))
            .frame(width: 84, height: 56)
    }

    /// 22 pt grid of hairlines (the hero's two linear-gradient backgrounds).
    private var gridLines: some View {
        Canvas { context, size in
            var path = Path()
            var x: CGFloat = 0
            while x <= size.width {
                path.addRect(CGRect(x: x, y: 0, width: 1, height: size.height))
                x += 22
            }
            var y: CGFloat = 0
            while y <= size.height {
                path.addRect(CGRect(x: 0, y: y, width: size.width, height: 1))
                y += 22
            }
            context.fill(path, with: .color(Color("BorderStrong")))
        }
    }
}

/// `.phone-pair-nudge.phone-promo-sheet`: the pair / teach suggestion card
/// inside the ACI canvas — hero first, close over it, title, one line, one
/// pill action.
struct ACINudgeCard: View {
    let hero: ACIPromoHero.Kind
    let title: String
    let message: String
    let closeLabel: String
    let actionTitle: String
    let onClose: () -> Void
    let onAction: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ACIPromoHero(kind: hero)
                .padding(.horizontal, -20)
                .overlay(alignment: .topLeading) {
                    Button(action: onClose) {
                        Image(systemName: "xmark")
                            .font(.system(size: 17))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(width: 40, height: 40)
                            .background(Color("BgPrimary"))
                            .clipShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .padding(.leading, -4)
                    .padding(.top, 16)
                    .accessibilityLabel(closeLabel)
                }
            Text(title)
                .font(.system(size: 26, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .accessibilityAddTraits(.isHeader)
            Text(message)
                .font(.system(size: 17))
                .lineSpacing(4)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            ACIPrimaryButton(title: actionTitle, action: onAction)
        }
        .padding(.horizontal, 20)
        .padding(.bottom, 20)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .shadow(color: Color.black.opacity(0.15), radius: 20, x: 0, y: -10)
        .padding(12)
    }
}

// MARK: - Empty state

/// The dashed empty card (MachinesPanel / `.phone-phones-empty`).
struct ACIDashedCard<Content: View>: View {
    let radius: CGFloat
    let content: () -> Content

    init(radius: CGFloat = 16, @ViewBuilder content: @escaping () -> Content) {
        self.radius = radius
        self.content = content
    }

    var body: some View {
        content()
            .frame(maxWidth: .infinity)
            .overlay(
                RoundedRectangle(cornerRadius: radius)
                    .strokeBorder(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
            )
    }
}
