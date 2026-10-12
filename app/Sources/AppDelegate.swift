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

    /// SANDBOX SPIKE: files opened through Launch Services (`open -a`, the Dock
    /// icon). The mode comes from the filename prefix so a test needs no input.
    func application(_ application: NSApplication, open urls: [URL]) {
        NSLog("SPIKE open %ld urls", urls.count)
        Task { @MainActor in
            for url in urls {
                let name = url.lastPathComponent
                let mode: Engine.Mode = name.hasPrefix("strip-") ? .strip
                    : name.hasPrefix("quality-") ? .quality : .fast
                JobQueue.shared.add([url], mode: mode)
            }
        }
    }

    /// Keep running when the window is closed: the Finder service must still work.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }
}
