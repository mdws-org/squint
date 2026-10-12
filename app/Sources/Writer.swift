import Foundation

/// Replaces a file's contents without losing what the filesystem was holding.
///
/// A naive write destroys Finder tags, which live in extended attributes, along
/// with the creation date. `replaceItemAt` preserves them, because it is designed
/// for exactly this: swapping contents while keeping the original's identity.
///
/// SANDBOX SPIKE: tries each strategy in turn and logs which ones the App
/// Sandbox allows, so one run answers the question for all three.
enum Writer {
    enum Strategy: String, CaseIterable {
        /// The shipping strategy: a temporary file beside the original.
        case sameDirectory
        /// A temporary file in the directory macOS hands out for replacing
        /// `url`, which a sandboxed app is allowed to create.
        case replacementDirectory
        /// Truncate and rewrite the original through a handle. Not atomic.
        case overwrite
    }

    static func replaceInPlace(_ url: URL, with data: Data) throws {
        var lastError: Error?
        for strategy in Strategy.allCases {
            do {
                try write(data, over: url, using: strategy)
                NSLog("SPIKE writer=%@ ok %@", strategy.rawValue, url.path)
                return
            } catch {
                NSLog("SPIKE writer=%@ FAILED %@: %@", strategy.rawValue, url.path, String(describing: error))
                lastError = error
            }
        }
        throw lastError!
    }

    private static func write(_ data: Data, over url: URL, using strategy: Strategy) throws {
        switch strategy {
        case .sameDirectory:
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
        case .replacementDirectory:
            let directory = try FileManager.default.url(
                for: .itemReplacementDirectory,
                in: .userDomainMask,
                appropriateFor: url,
                create: true
            )
            defer { try? FileManager.default.removeItem(at: directory) }
            NSLog("SPIKE replacementDirectory=%@", directory.path)
            let temporary = directory.appendingPathComponent(url.lastPathComponent)
            try data.write(to: temporary)
            _ = try FileManager.default.replaceItemAt(url, withItemAt: temporary)
        case .overwrite:
            let handle = try FileHandle(forWritingTo: url)
            defer { try? handle.close() }
            try handle.truncate(atOffset: 0)
            try handle.write(contentsOf: data)
        }
    }

    /// Finder tags, read back so a caller can verify they survived.
    static func tags(of url: URL) -> [String] {
        (try? url.resourceValues(forKeys: [.tagNamesKey]))?.tagNames ?? []
    }
}
