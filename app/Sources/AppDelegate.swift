import AppKit

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let services = ServiceProvider()

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.servicesProvider = services
        // Ask the pasteboard server to re-read this application's Info.plist.
        // Without it a freshly built application does not appear in the menu
        // until something else triggers a rescan.
        NSUpdateDynamicServices()
    }

    /// Keep running when the window is closed: the Finder service must still work.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    /// A click on the Dock icon with no window open brings the window back.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag {
            MainWindow.show()
        }
        return true
    }
}
