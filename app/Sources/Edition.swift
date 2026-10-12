import Foundation

/// Which build this is.
///
/// The Mac App Store edition is sandboxed, and the sandbox grants an
/// application the file it was handed but not the folder around it. So that
/// edition offers only what replaces a file in place, and leaves the copies
/// written beside the original to the edition on the releases page. It has no
/// updater of its own, since the App Store updates it.
enum Edition {
    #if APP_STORE
    static let isStore = true
    #else
    static let isStore = false
    #endif

    /// Where the full edition is downloaded.
    static let fullEditionURL = URL(string: "https://github.com/mdws-org/squint/releases/latest")!
}
