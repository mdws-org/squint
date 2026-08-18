import AppKit

/// Handles the Finder right-click entry.
///
/// This is the way the application is actually used. Files arrive from Finder's
/// Services menu rather than by being dragged onto a window, so this path has to
/// work without the window ever being opened.
final class ServiceProvider: NSObject {
    /// Named by `NSMessage` in the Info.plist. Renaming this breaks the menu item
    /// silently, with no error at registration time.
    @objc func optimizeFiles(
        _ pasteboard: NSPasteboard,
        userData: String?,
        error: AutoreleasingUnsafeMutablePointer<NSString>
    ) {
        let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: nil) as? [URL] ?? []
        let images = urls.filter { Self.isSupported($0) }

        guard !images.isEmpty else {
            error.pointee = "No JPEG or PNG images were selected." as NSString
            return
        }

        Task { @MainActor in
            // Bring the window forward so progress is visible. Work started from
            // Finder is otherwise invisible until it finishes.
            NSApp.activate(ignoringOtherApps: true)
            JobQueue.shared.add(images)
        }
    }

    private static func isSupported(_ url: URL) -> Bool {
        ["jpg", "jpeg", "png"].contains(url.pathExtension.lowercased())
    }
}
