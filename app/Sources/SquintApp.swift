import SwiftUI

@main
struct SquintApp: App {
    var body: some Scene {
        Window("squint", id: "main") {
            ContentView()
        }
        .windowResizability(.contentSize)
    }
}
