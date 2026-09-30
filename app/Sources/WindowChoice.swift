import Foundation

/// What a drop onto the window does: the same six treatments the Finder
/// entries offer.
///
/// The window has to reach everything the right-click menu reaches. A photo in
/// a folder Finder does not offer the Services entries for, or one the person
/// would rather drag than right-click, otherwise has no route to the email or
/// social copy at all.
enum WindowChoice: String, CaseIterable, Identifiable {
    case shrink, email, social, quality, avif, strip

    var id: String { rawValue }

    /// Short enough to sit side by side in the window's picker.
    var label: String {
        switch self {
        case .shrink: return "Shrink"
        case .email: return "Email"
        case .social: return "Social"
        case .quality: return "Quality"
        case .avif: return "AVIF"
        case .strip: return "Strip"
        }
    }

    var preset: Preset {
        switch self {
        case .shrink: return .plain(.fast)
        case .email: return .email
        case .social: return .social
        case .quality: return .plain(.quality)
        case .avif: return .avif
        case .strip: return .plain(.strip)
        }
    }

    /// Whether the quality target applies. AVIF is searched to it as the
    /// Quality entry is.
    var usesTarget: Bool { self == .quality || self == .avif }

    /// One line under the drop prompt, saying what happens to a dropped file.
    var summary: String {
        switch self {
        case .shrink: return "JPEG, PNG and PDF, replaced in place."
        case .email: return "Writes name-email.jpg at 2048 pixels beside the original."
        case .social: return "Writes name-social.jpg at 1440 pixels beside the original."
        case .quality: return "JPEG, PNG and PDF, the smallest file that meets the target, replaced in place."
        case .avif: return "Writes name.avif beside the original, searched to the target."
        case .strip: return "Removes location, camera and date. Pixels are untouched."
        }
    }
}
