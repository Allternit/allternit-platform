import WidgetKit
import SwiftUI

@main
struct AllternitWidgetsBundle: WidgetBundle {
    var body: some Widget {
        LoopLiveActivityWidget()
        BotLiveActivityWidget()
        StatusWidget()
    }
}
