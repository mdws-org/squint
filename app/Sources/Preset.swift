import Foundation

/// A named destination: everything the engine needs to know, decided in advance.
///
/// The Finder menu offers destinations, not settings. Whatever a destination
/// needs — a mode, a size cap, whether the original survives — belongs in here,
/// so that nothing is decided at the moment of use.
struct Preset {
    let mode: Engine.Mode
    /// Long-edge cap in pixels, or nil for the picture's own size.
    let maxDimension: Int32?
    /// Appended to the filename when the result is written beside the original
    /// rather than over it. A preset that resizes writes beside: the cap throws
    /// resolution away, and a photograph kept as documentation should not lose
    /// it because a copy was being made for an email.
    let suffix: String?

    /// The three plain modes, replacing in place as they always have.
    static func plain(_ mode: Engine.Mode) -> Preset {
        Preset(mode: mode, maxDimension: nil, suffix: nil)
    }

    /// Shrink for email: 2048 pixels on the long edge, written beside the
    /// original as `name-email.jpg`.
    ///
    /// Measured on a 4032x3024 photograph: 129 KB, so about thirty photographs
    /// fit under any provider's attachment limit, and the picture stays sharp
    /// on every phone and laptop it will be seen on. 2048 rather than smaller
    /// because a client zooms into exactly the detail a job photograph is sent
    /// to show.
    static let email = Preset(mode: .fast, maxDimension: 2048, suffix: "-email")

    /// Where this preset's output goes for a given input.
    func destination(for url: URL) -> URL {
        guard let suffix else { return url }
        let stem = url.deletingPathExtension().lastPathComponent
        return url
            .deletingLastPathComponent()
            .appendingPathComponent(stem + suffix)
            .appendingPathExtension(url.pathExtension)
    }
}
