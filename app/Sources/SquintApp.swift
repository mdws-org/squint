#if !APP_STORE
import Sparkle
#endif
import SwiftUI

@main
struct SquintApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    #if !APP_STORE
    /// Started with the application. Sparkle then checks on its own schedule
    /// and the menu item below asks for a check on demand. Held here because
    /// the updater must outlive any one window: this application spends most
    /// of its life with no window open, answering the Finder services.
    ///
    /// The store edition has none of this: the App Store updates it.
    private let updaterController = SPUStandardUpdaterController(
        startingUpdater: true,
        updaterDelegate: nil,
        userDriverDelegate: nil
    )
    #endif

    var body: some Scene {
        Window("Squint", id: MainWindow.id) {
            ContentView()
                .capturesItsOpener()
        }
        .windowResizability(.contentSize)
        #if !APP_STORE
        .commands {
            CommandGroup(after: .appInfo) {
                CheckForUpdatesView(updater: updaterController.updater)
            }
        }
        #endif
    }
}
