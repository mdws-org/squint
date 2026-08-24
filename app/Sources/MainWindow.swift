import AppKit
import SwiftUI

/// Brings the application's window back, from code that is not a view.
///
/// The Finder services are the primary way this application is used, and they
/// run whether or not a window is open. `NSApp.activate` raises an application;
/// it does not reopen a `Window` scene that has been closed, and closing that
/// scene destroys its `NSWindow`. So a right-click after the window was closed
/// rewrote files with nothing on screen: no progress, no score, no error.
///
/// SwiftUI will only reopen the scene through its own action, which exists in
/// the environment of a view. The action is a value and outlives the view it
/// came from, so the window captures it once and this holds it for the service
/// to use later.
@MainActor
enum MainWindow {
    static let id = "main"

    /// Captured by the window itself. Absent only before the first launch has
    /// drawn anything, when there is a window on screen already.
    static var reopen: (() -> Void)?

    static func show() {
        NSApp.activate(ignoringOtherApps: true)
        if let existing = NSApp.windows.first(where: { $0.canBecomeMain && !$0.isMiniaturized }) {
            existing.makeKeyAndOrderFront(nil)
            return
        }
        reopen?()
    }
}

/// Records the scene's opener while the window is on screen.
struct CapturesItsOpener: ViewModifier {
    @Environment(\.openWindow) private var openWindow

    func body(content: Content) -> some View {
        content.onAppear {
            MainWindow.reopen = { openWindow(id: MainWindow.id) }
        }
    }
}

extension View {
    func capturesItsOpener() -> some View {
        modifier(CapturesItsOpener())
    }
}
