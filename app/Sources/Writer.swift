import Foundation

/// Replaces a file's contents without losing what the filesystem was holding.
///
/// A naive write destroys Finder tags, which live in extended attributes, along
/// with the creation date. `replaceItemAt` preserves them, because it is designed
/// for exactly this: swapping contents while keeping the original's identity.
enum Writer {
    /// Write `data` over `url`, keeping tags, creation date, and permissions.
    ///
    /// The temporary file goes in the directory macOS hands out for replacing
    /// `url`. It is on the same volume, so the swap is a rename and cannot
    /// become a slow cross-device copy, and it is somewhere the App Sandbox
    /// lets the application create a file. The original's own directory is
    /// not: a sandboxed application is granted the file it was given, never
    /// the folder around it.
    static func replaceInPlace(_ url: URL, with data: Data) throws {
        let directory = try FileManager.default.url(
            for: .itemReplacementDirectory,
            in: .userDomainMask,
            appropriateFor: url,
            create: true
        )
        // Made for this one replacement, and removed whether or not it worked.
        defer { try? FileManager.default.removeItem(at: directory) }
        let temporary = directory.appendingPathComponent(url.lastPathComponent)

        try data.write(to: temporary)

        // Passing no options means the original's metadata is retained.
        // Passing .usingNewMetadataOnly would discard the tags.
        _ = try FileManager.default.replaceItemAt(url, withItemAt: temporary)
    }

    /// Finder tags, read back so a caller can verify they survived.
    static func tags(of url: URL) -> [String] {
        (try? url.resourceValues(forKeys: [.tagNamesKey]))?.tagNames ?? []
    }
}
