import AppKit
import FinderSync

final class AppDelegate: NSObject, NSApplicationDelegate {
    private static let askedKey = "askedToTurnOnFinderMenu"

    func applicationWillFinishLaunching(_ notification: Notification) {
        // The Finder extension wakes the application with a squint:// link.
        // Taken as an Apple event, before SwiftUI sees it, because SwiftUI
        // answers an opened URL by opening another window.
        NSAppleEventManager.shared().setEventHandler(
            self,
            andSelector: #selector(openLink(_:reply:)),
            forEventClass: AEEventClass(kInternetEventClass),
            andEventID: AEEventID(kAEGetURL)
        )
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Asked once. macOS keeps a Finder extension switched off until the
        // person turns it on, and nothing in Finder says so: the submenu is
        // simply absent. The same switch is reachable later from the Squint menu.
        if !UserDefaults.standard.bool(forKey: Self.askedKey) {
            UserDefaults.standard.set(true, forKey: Self.askedKey)
            Self.offerFinderMenu()
        }
    }

    @objc private func openLink(_ event: NSAppleEventDescriptor, reply: NSAppleEventDescriptor) {
        guard let text = event.paramDescriptor(forKeyword: keyDirectObject)?.stringValue,
              let link = URL(string: text)
        else { return }
        FinderRequests.accept(link)
    }

    /// Keep running when the window is closed: the Finder menu must still work.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    @MainActor
    static func offerFinderMenu() {
        let alert = NSAlert()
        alert.messageText = "Turn on Squint in Finder"
        alert.informativeText = "Squint's entries in Finder's right-click menu come from a Finder extension, which macOS keeps switched off until you turn it on. System Settings opens at the list of extensions: switch on Squint, and a Squint submenu appears when you right-click an image."
        alert.addButton(withTitle: "Open System Settings")
        alert.addButton(withTitle: "Not Now")
        if alert.runModal() == .alertFirstButtonReturn {
            FIFinderSyncController.showExtensionManagementInterface()
        }
    }
}
