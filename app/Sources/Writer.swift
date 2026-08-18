import Foundation

/// Replaces a file's contents without losing what the filesystem was holding.
///
/// A naive write destroys Finder tags, which live in extended attributes, along
/// with the creation date. `replaceItemAt` preserves them, because it is designed
/// for exactly this: swapping contents while keeping the original's identity.
enum Writer {
    /// Write `data` over `url`, keeping tags, creation date, and permissions.
    ///
    /// The temporary file is created in the same directory so the replacement
    /// stays on one volume and cannot become a slow cross-device copy.
    static func replaceInPlace(_ url: URL, with data: Data) throws {
        let directory = url.deletingLastPathComponent()
        let temporary = directory.appendingPathComponent(
            ".squint-\(UUID().uuidString).tmp"
        )

        try data.write(to: temporary, options: .atomic)

        do {
            // Passing no options means the original's metadata is retained.
            // Passing .usingNewMetadataOnly would discard the tags.
            _ = try FileManager.default.replaceItemAt(url, withItemAt: temporary)
        } catch {
            try? FileManager.default.removeItem(at: temporary)
            throw error
        }
    }

    /// Finder tags, read back so a caller can verify they survived.
    static func tags(of url: URL) -> [String] {
        (try? url.resourceValues(forKeys: [.tagNamesKey]))?.tagNames ?? []
    }
}
