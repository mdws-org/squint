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
    let preset: Preset
    let target: Double
    @Published var state: State = .waiting

    var mode: Engine.Mode { preset.mode }

    init(url: URL, preset: Preset, target: Double) {
        self.url = url
        self.preset = preset
        self.target = target
    }

    var name: String { url.lastPathComponent }

    var detail: String {
        switch state {
        case .waiting: return "waiting"
        case .working: return "working"
        // Strip found nothing to take out, which is a statement about privacy
        // rather than size. "Already optimal" would be the wrong claim: a file
        // can be as small as it will go and still carry a location.
        case .alreadyOptimal:
            return mode == .strip ? "nothing to remove" : "already optimal"
        case .failed(let message): return message
        case .done(let bytes, let original, let score, let hdr, let quantized):
            let saved = 100 - (100 * Double(bytes) / Double(original))
            // Strip is run to answer a question about the file's contents, not
            // its size, so it leads with what came out. A byte count is the
            // wrong headline for a mode whose whole point is that the pixels
            // did not change.
            var text = mode == .strip
                ? "location and camera data removed"
                : "\(format(original)) to \(format(bytes)), \(Int(saved.rounded()))% smaller"
            if let score {
                text += String(format: ", score %.1f (%@)", score, Self.band(score))
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
            if mode == .strip {
                text += ", \(format(bytes))"
            }
            // A preset that wrote beside the original says where, since the
            // point of the run is the new file rather than the old one.
            if preset.suffix != nil {
                text = "wrote \(preset.destination(for: url).lastPathComponent), " + text
            }
            return text
        }
    }

    /// What a SSIMULACRA2 score means, since the number alone says nothing to
    /// anyone who has not read the metric's calibration.
    private static func band(_ score: Double) -> String {
        switch score {
        case 90...: return "visually lossless"
        case 80..<90: return "high"
        case 70..<80: return "good for web"
        default: return "low"
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
