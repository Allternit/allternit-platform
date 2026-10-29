import SwiftUI

/// Automation's root: the phone Automation Tasks screen (goals, routines,
/// loops and heartbeats in one list). Kept under its old name for callers.
struct AutomationOverviewRoot: View {
    @Binding var isSidebarOpen: Bool

    var body: some View {
        AutomationTasksPhoneView(isSidebarOpen: $isSidebarOpen)
    }
}
