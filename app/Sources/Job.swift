import Foundation

/// One file moving through the queue.
@MainActor
final class Job: ObservableObject, Identifiable {
    enum State: Equatable {
        case waiting
        case working
        /// Finished and smaller.
        case done(bytes: Int, originalBytes: Int, score: Double?, hdr: Engine.Hdr, quantized: Bool)
        /// Finished, but the file was already as small as it can be.
        case alreadyOptimal
        case failed(String)
    }

    let id = UUID()
    let url: URL
    /// Fixed when the job is queued. A right-click carries its own instruction,
    /// and the window's picker must not be able to change it afterwards.
    let mode: Engine.Mode
    let target: Double
    @Published var state: State = .waiting

    init(url: URL, mode: Engine.Mode, target: Double) {
        self.url = url
        self.mode = mode
        self.target = target
    }

    var name: String { url.lastPathComponent }

    var detail: String {
        switch state {
        case .waiting: return "waiting"
        case .working: return "working"
        case .alreadyOptimal: return "already optimal"
        case .failed(let message): return message
        case .done(let bytes, let original, let score, let hdr, let quantized):
            let saved = 100 - (100 * Double(bytes) / Double(original))
            var text = "\(format(original)) to \(format(bytes)), \(Int(saved.rounded()))% smaller"
            if let score {
                text += String(format: ", score %.1f", score)
            }
            // Losing the extra range changes how the picture looks on a display
            // that can show it, so it is said out loud rather than left to be
            // noticed later.
            switch hdr {
            case .absent: break
            case .preserved: text += ", HDR kept"
            case .dropped: text += ", HDR removed"
            }
            // PNG's lossy mode reduces the colour count rather than turning a
            // quality dial, and it is on by default. Saying so is the difference
            // between a smaller file and a changed picture.
            if quantized {
                text += ", colours reduced"
            }
            return text
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
