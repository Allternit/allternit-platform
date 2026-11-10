import WidgetKit
import SwiftUI

@main
struct AllternitWidgetsBundle: WidgetBundle {
    var body: some Widget {
        LoopLiveActivityWidget()
        BotLiveActivityWidget()
        StatusWidget()
        if #available(iOS 18.0, *) {
            CallMainBotControl()
            AskBotControl()
            HandOffTaskControl()
            RunRoutineControl()
            NeedsMeControl()
            PauseBotsControl()
        }
    }
}
