import AppKit

/// Handles the Finder right-click entries.
///
/// This is how the application is actually used: files arrive from the Services
/// menu rather than by being dragged onto a window, so this path works whether or
/// not the window is open.
///
/// There is one entry per mode rather than one entry that follows the window's
/// setting. A menu item whose behaviour depends on hidden state elsewhere is not
/// something a person can predict from Finder, where the window is not visible.
final class ServiceProvider: NSObject {
    /// Named by `NSMessage` in the Info.plist. Renaming this breaks the menu item
    /// silently: it still appears, and does nothing, with no registration error.
    @objc func optimizeFast(
        _ pasteboard: NSPasteboard,
        userData: String?,
        error: AutoreleasingUnsafeMutablePointer<NSString>
    ) {
        run(pasteboard, mode: .fast, error: error)
    }

    @objc func optimizeQuality(
        _ pasteboard: NSPasteboard,
        userData: String?,
        error: AutoreleasingUnsafeMutablePointer<NSString>
    ) {
        run(pasteboard, mode: .quality, error: error)
    }

    @objc func stripMetadata(
        _ pasteboard: NSPasteboard,
        userData: String?,
        error: AutoreleasingUnsafeMutablePointer<NSString>
    ) {
        run(pasteboard, mode: .strip, error: error)
    }

    private func run(
        _ pasteboard: NSPasteboard,
        mode: Engine.Mode,
        error: AutoreleasingUnsafeMutablePointer<NSString>
    ) {
        let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: nil) as? [URL] ?? []
        let images = urls.filter { Self.isSupported($0) }

        guard !images.isEmpty else {
            error.pointee = "No JPEG or PNG images were selected." as NSString
            return
        }

        Task { @MainActor in
            // Put the window on screen before queueing. Work started from Finder
            // is otherwise invisible: the application keeps running with no
            // window so the services stay available, and activating it alone
            // shows nothing at all.
            MainWindow.show()
            // The mode goes with the files rather than onto the queue. Setting
            // it here used to move the window's picker too, so the next file
            // dropped on the window was treated as whatever was last
            // right-clicked.
            JobQueue.shared.add(images, mode: mode)
        }
    }

    private static func isSupported(_ url: URL) -> Bool {
        ["jpg", "jpeg", "png"].contains(url.pathExtension.lowercased())
    }
}
