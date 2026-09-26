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

    @objc func shrinkForEmail(
        _ pasteboard: NSPasteboard,
        userData: String?,
        error: AutoreleasingUnsafeMutablePointer<NSString>
    ) {
        run(pasteboard, preset: .email, error: error)
    }

    @objc func shrinkForSocial(
        _ pasteboard: NSPasteboard,
        userData: String?,
        error: AutoreleasingUnsafeMutablePointer<NSString>
    ) {
        run(pasteboard, preset: .social, error: error)
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

    @objc func convertToAvif(
        _ pasteboard: NSPasteboard,
        userData: String?,
        error: AutoreleasingUnsafeMutablePointer<NSString>
    ) {
        run(pasteboard, preset: .avif, error: error)
    }

    private func run(
        _ pasteboard: NSPasteboard,
        mode: Engine.Mode,
        error: AutoreleasingUnsafeMutablePointer<NSString>
    ) {
        run(pasteboard, preset: .plain(mode), error: error)
    }

    private func run(
        _ pasteboard: NSPasteboard,
        preset: Preset,
        error: AutoreleasingUnsafeMutablePointer<NSString>
    ) {
        let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: nil) as? [URL] ?? []
        let images = urls.filter { Self.isSupported($0, by: preset) }

        guard !images.isEmpty else {
            error.pointee = "Nothing selected is a kind of file this entry accepts." as NSString
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
            JobQueue.shared.add(images, preset: preset)
        }
    }

    /// A second filter behind Finder's own, since a service can be invoked with
    /// a selection Finder was willing to pass along.
    ///
    /// HEIC, AVIF, WebP, TIFF and GIF belong here because what they disclose
    /// can be removed. TIFF and GIF are offered to no shrinking entry, and the
    /// formats the engine can only re-encode as JPEG — HEIC, AVIF, WebP and
    /// SVG — only to the entries that write beside the original, since a JPEG
    /// must not be written over a file that is not one. A PDF is rewritten as
    /// a PDF, with the pictures inside it re-encoded or its own record of who
    /// wrote it dropped, so it is accepted by the three in-place entries and
    /// by nothing that writes a picture beside the original: the filter asks
    /// the preset, so that a PDF reaching an entry Finder never offers it to
    /// is turned away here rather than rewritten under a name like
    /// `report-email.pdf`.
    ///
    /// This list and the `NSSendFileTypes` in `project.yml` have to be changed
    /// together. Finder decides from the type declarations whether the menu
    /// item appears at all; this decides whether the work is done once it has.
    /// A format in one and not the other either never shows up or shows up and
    /// refuses.
    private static func isSupported(_ url: URL, by preset: Preset) -> Bool {
        let ext = url.pathExtension.lowercased()
        if ext == "pdf" { return preset.writesInPlace }
        return ["jpg", "jpeg", "png", "heic", "heif", "avif", "webp", "svg", "tif", "tiff", "gif"]
            .contains(ext)
    }
}
