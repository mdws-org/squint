import AppKit

/// Takes a choice made in Squint's Finder submenu and does the work.
///
/// The link that wakes the application carries only a token. Which entry was
/// chosen and which files it was chosen for are read from the request the
/// extension left in the app group's folder, which is deleted as it is read.
/// A `squint://` link from a web page or another application names a token
/// with no request behind it, and does nothing: a link alone cannot make
/// Squint rewrite a file.
enum FinderRequests {
    /// A request older than this is ignored. The extension writes it and opens
    /// the link in the same moment; anything older was not made by a click.
    private static let freshness: TimeInterval = 60

    static func accept(_ link: URL) {
        guard link.scheme == Handoff.scheme, link.host == "run",
              let token = URLComponents(url: link, resolvingAgainstBaseURL: false)?
                  .queryItems?.first(where: { $0.name == "token" })?.value,
              UUID(uuidString: token) != nil,
              let folder = Handoff.requests
        else { return }

        let file = folder.appendingPathComponent("\(token).json")
        guard let made = (try? FileManager.default.attributesOfItem(atPath: file.path))?[.modificationDate] as? Date,
              Date().timeIntervalSince(made) < freshness,
              let data = try? Data(contentsOf: file)
        else { return }
        try? FileManager.default.removeItem(at: file)

        guard let request = try? JSONDecoder().decode(Handoff.Request.self, from: data),
              let entry = FinderEntry(rawValue: request.entry)
        else { return }
        // Checked again here, since the file names came from outside this
        // process: an entry only ever works on the kinds of file it offers.
        let files = request.paths.map { URL(fileURLWithPath: $0) }.filter { entry.acceptsAll([$0]) }
        guard !files.isEmpty else { return }

        Task { @MainActor in
            // The window goes on screen before the work is queued. Work started
            // from Finder is otherwise invisible: the application keeps running
            // with no window, and activating it alone shows nothing at all.
            MainWindow.show()
            JobQueue.shared.add(files, preset: entry.preset)
        }
    }
}

extension FinderEntry {
    var preset: Preset {
        switch self {
        case .shrink: return .plain(.fast)
        case .email: return .email
        case .social: return .social
        case .quality: return .plain(.quality)
        case .avif: return .avif
        case .removeLocation: return .plain(.strip)
        }
    }
}
