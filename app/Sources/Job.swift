import Foundation

/// One file moving through the queue.
@MainActor
final class Job: ObservableObject, Identifiable {
    enum State: Equatable {
        case waiting
        case working
        /// Finished and smaller.
        case done(bytes: Int, originalBytes: Int, score: Double?)
        /// Finished, but the file was already as small as it can be.
        case alreadyOptimal
        case failed(String)
    }

    let id = UUID()
    let url: URL
    @Published var state: State = .waiting

    init(url: URL) {
        self.url = url
    }

    var name: String { url.lastPathComponent }

    var detail: String {
        switch state {
        case .waiting: return "waiting"
        case .working: return "working"
        case .alreadyOptimal: return "already optimal"
        case .failed(let message): return message
        case .done(let bytes, let original, let score):
            let saved = 100 - (100 * Double(bytes) / Double(original))
            let sizes = "\(format(original)) to \(format(bytes)), \(Int(saved.rounded()))% smaller"
            guard let score else { return sizes }
            return sizes + String(format: ", score %.1f", score)
        }
    }

    private func format(_ bytes: Int) -> String {
        let units = ["B", "KB", "MB"]
        var value = Double(bytes)
        var unit = 0
        while value >= 1024, unit < units.count - 1 {
            value /= 1024
            unit += 1
        }
        return String(format: unit == 0 ? "%.0f %@" : "%.1f %@", value, units[unit])
    }
}
