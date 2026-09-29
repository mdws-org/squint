import Foundation

/// The entries in Squint's Finder submenu, compiled into both the application
/// and its Finder extension.
///
/// The extension decides from this list which entries to offer for a
/// selection, and the application decides from the same list whether to do
/// the work, so the two cannot disagree about which files an entry accepts.
/// An entry is offered only when every selected item is a file it accepts; a
/// folder, or a HEIC mixed into a batch for an in-place entry, leaves it out.
enum FinderEntry: Int, CaseIterable {
    case shrink, email, social, quality, avif, removeLocation

    var title: String {
        switch self {
        case .shrink: return "Shrink"
        case .email: return "Shrink for Email"
        case .social: return "Shrink for Social"
        case .quality: return "Shrink to a Quality Target"
        case .avif: return "Convert to AVIF"
        case .removeLocation: return "Remove Location Data"
        }
    }

    /// Filename extensions, lowercased.
    ///
    /// The in-place entries take JPEG, PNG and PDF: squint writes JPEG, and a
    /// JPEG must not overwrite a file that is not one, while a PDF is rewritten
    /// as a PDF. The entries that write beside the original also take the
    /// formats the engine can only re-encode. Remove Location Data takes every
    /// format whose metadata the engine can strip, which is every format it
    /// reads except SVG: a drawing carries no metadata block to remove.
    var accepts: Set<String> {
        let raster: Set<String> = ["jpg", "jpeg", "png"]
        switch self {
        case .shrink, .quality: return raster.union(["pdf"])
        case .email, .social: return raster.union(["heic", "heif", "avif", "webp", "svg"])
        case .avif: return raster.union(["heic", "heif", "webp"])
        case .removeLocation: return raster.union(["heic", "heif", "avif", "webp", "tif", "tiff", "gif", "pdf"])
        }
    }

    func acceptsAll(_ urls: [URL]) -> Bool {
        !urls.isEmpty && urls.allSatisfy { !$0.hasDirectoryPath && accepts.contains($0.pathExtension.lowercased()) }
    }
}

/// How a choice made in Finder reaches the application.
///
/// The extension is sandboxed and cannot open the files. It writes the choice
/// into the app group's folder and wakes the application with a link naming
/// only that request; the application, which is not sandboxed, reads the
/// request, deletes it, and does the work.
enum Handoff {
    static let appGroup = "CY9SLX2KFM.net.mdws.squint"
    static let scheme = "squint"

    static var requests: URL? {
        FileManager.default
            .containerURL(forSecurityApplicationGroupIdentifier: appGroup)?
            .appendingPathComponent("requests", isDirectory: true)
    }

    struct Request: Codable {
        let entry: Int
        let paths: [String]
    }
}
