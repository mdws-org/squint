import Sparkle
import SwiftUI

@main
struct SquintApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    /// Started with the application. Sparkle then checks on its own schedule
    /// and the menu item below asks for a check on demand. Held here because
    /// the updater must outlive any one window: this application spends most
    /// of its life with no window open, answering the Finder menu.
    private let updaterController = SPUStandardUpdaterController(
        startingUpdater: true,
        updaterDelegate: nil,
        userDriverDelegate: nil
    )

    var body: some Scene {
        Window("Squint", id: MainWindow.id) {
            ContentView()
                .capturesItsOpener()
        }
        .windowResizability(.contentSize)
        .commands {
            CommandGroup(after: .appInfo) {
                CheckForUpdatesView(updater: updaterController.updater)
                Button("Turn On Finder Menu…") { AppDelegate.offerFinderMenu() }
            }
        }
    }
}
