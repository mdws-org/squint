import SwiftUI

@main
struct SquintApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        Window("Squint", id: "main") {
            ContentView()
        }
        .windowResizability(.contentSize)
    }
}
