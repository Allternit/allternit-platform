import SwiftUI

/// Status dot for the four presence states.
struct UserStatusDot: View {
    let status: UserStatus

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: 8, height: 8)
            .accessibilityHidden(true)
    }

    private var color: Color {
        switch status {
        case .online: return .green
        case .autopilot: return .blue
        case .dnd: return .red
        case .away: return .gray
        }
    }
}

/// Loads and saves the owner's status; shared by the profile Picker and the
/// account row menu.
@MainActor
final class UserStatusModel: ObservableObject {
    @Published var status: UserStatus = .online

    func load() async {
        if let value = try? await UserStatusClient.shared.get() { status = value }
    }

    func choose(_ next: UserStatus) {
        let previous = status
        status = next
        Task {
            do { try await UserStatusClient.shared.set(next) } catch { status = previous }
        }
    }
}

/// The account row at the foot of Settings: "Name · Status" with a dot; the
/// menu holds the 4 statuses and "Account settings".
struct UserStatusAccountMenu<Avatar: View>: View {
    let name: String
    let onAccountSettings: () -> Void
    @ViewBuilder let avatar: () -> Avatar
    @StateObject private var model = UserStatusModel()

    var body: some View {
        Menu {
            ForEach(UserStatus.allCases) { item in
                Button {
                    model.choose(item)
                } label: {
                    if model.status == item {
                        Label(item.title, systemImage: "checkmark")
                    } else {
                        Text(item.title)
                    }
                    if !item.subline.isEmpty { Text(item.subline) }
                }
            }
            Divider()
            Button("Account settings", action: onAccountSettings)
        } label: {
            HStack(spacing: 12) {
                avatar()
                HStack(spacing: 6) {
                    Text(name)
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text("·").foregroundColor(Color("TextSecondary"))
                    UserStatusDot(status: model.status)
                    Text(model.status.title)
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .frame(minHeight: 56)
            .contentShape(Rectangle())
        }
        .accessibilityLabel("\(name), \(model.status.title)")
        .task { await model.load() }
    }
}
