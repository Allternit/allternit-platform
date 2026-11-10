import WidgetKit
import SwiftUI

/// Picks the bundle by OS: a `WidgetBundleBuilder` takes `if #available` but
/// not `else`, and the status widget must appear once (with the push handler
/// on iOS 26, without it before).
@main
enum AllternitWidgetsMain {
    static func main() {
        if #available(iOS 26.0, *) {
            AllternitWidgetsBundlePush.main()
        } else {
            AllternitWidgetsBundle.main()
        }
    }
}

struct AllternitWidgetsBundle: WidgetBundle {
    var body: some Widget {
        LoopLiveActivityWidget()
        BotLiveActivityWidget()
        StatusWidget()
        if #available(iOS 18.0, *) {
            AllternitControlsBundle().body
        }
    }
}

@available(iOS 26.0, *)
struct AllternitWidgetsBundlePush: WidgetBundle {
    var body: some Widget {
        LoopLiveActivityWidget()
        BotLiveActivityWidget()
        StatusWidgetWithPush()
        AllternitControlsBundle().body
    }
}

@available(iOS 18.0, *)
struct AllternitControlsBundle: WidgetBundle {
    var body: some Widget {
        CallMainBotControl()
        AskBotControl()
        HandOffTaskControl()
        RunRoutineControl()
        NeedsMeControl()
        PauseBotsControl()
    }
}
