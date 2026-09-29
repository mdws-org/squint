import Cocoa
import FinderSync

/// Squint's submenu in Finder's right-click menu.
///
/// Finder asks for the menu each time it is opened, so what is offered always
/// follows the current selection. The work is not done here: this runs
/// sandboxed inside Finder, and the files belong to whoever selected them.
final class FinderSync: FIFinderSync {
    override init() {
        super.init()
        // Every folder, as the Services entries were. Nothing is drawn on the
        // watched items, so watching everything costs Finder nothing.
        FIFinderSyncController.default().directoryURLs = [URL(fileURLWithPath: "/")]
    }

    override func menu(for menuKind: FIMenuKind) -> NSMenu? {
        guard menuKind == .contextualMenuForItems else { return nil }
        let selected = FIFinderSyncController.default().selectedItemURLs() ?? []
        let entries = FinderEntry.allCases.filter { $0.acceptsAll(selected) }
        guard !entries.isEmpty else { return nil }

        let submenu = NSMenu(title: "Squint")
        for entry in entries {
            // Finder copies the menu before showing it, keeping the tag but not
            // the represented object, so the entry travels as the tag.
            let item = NSMenuItem(title: entry.title, action: #selector(choose(_:)), keyEquivalent: "")
            item.tag = entry.rawValue
            submenu.addItem(item)
        }
        let root = NSMenuItem(title: "Squint", action: nil, keyEquivalent: "")
        root.submenu = submenu
        let menu = NSMenu(title: "")
        menu.addItem(root)
        return menu
    }

    @objc func choose(_ sender: NSMenuItem) {
        guard let entry = FinderEntry(rawValue: sender.tag), let folder = Handoff.requests else { return }
        let selected = FIFinderSyncController.default().selectedItemURLs() ?? []
        guard entry.acceptsAll(selected) else { return }

        let token = UUID().uuidString
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            let request = Handoff.Request(entry: entry.rawValue, paths: selected.map(\.path))
            try JSONEncoder().encode(request).write(to: folder.appendingPathComponent("\(token).json"), options: .atomic)
        } catch {
            NSLog("Squint Finder extension: could not write the request: \(error)")
            return
        }
        if let wake = URL(string: "\(Handoff.scheme)://run?token=\(token)") {
            NSWorkspace.shared.open(wake)
        }
    }
}
