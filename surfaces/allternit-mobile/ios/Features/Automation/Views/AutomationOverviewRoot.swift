import SwiftUI
import UIKit

/// Automation's default view: routine runs, calendar, graph and goals
/// (AutomationHubView) under the same header as the Cron / Routines / Loops
/// lists, whose kind menu switches between all four.
struct AutomationOverviewRoot: View {
    @Binding var isSidebarOpen: Bool
    @EnvironmentObject private var modeStore: AppModeStore

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                header
                AutomationHubView()
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .toolbar(.hidden, for: .navigationBar)
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Button(action: {
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
                    isSidebarOpen.toggle()
                }
            }) {
                Image(systemName: "line.3.horizontal")
                    .font(.title3)
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 44, height: 44)
            }
            .accessibilityLabel("Open sidebar")

            Menu {
                ForEach(AutomationKind.allCases, id: \.self) { kind in
                    Button(action: { modeStore.automationKind = kind }) {
                        Label(
                            kind.rawValue,
                            systemImage: modeStore.automationKind == kind ? "checkmark" : ""
                        )
                    }
                }
            } label: {
                HStack(spacing: 4) {
                    Text(modeStore.automationKind.rawValue)
                        .font(.system(.title3, design: .serif))
                        .fontWeight(.medium)
                        .foregroundColor(Color("TextPrimary"))
                    Image(systemName: "chevron.down")
                        .font(.system(size: 10, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
            }

            Spacer()
        }
        .padding(.horizontal, 8)
        .padding(.top, 4)
    }
}
