import Foundation

/// Swift face of the Rust engine.
///
/// Every buffer the engine returns is owned by Rust. This type is the only place
/// that knows it, and it always hands the buffer back.
enum Engine {
    enum Mode: Int32 {
        /// Encode once at a fixed quality, evaluate no metric.
        case fast = 0
        /// Search for the smallest file meeting a perceptual target.
        case quality = 1
        /// Remove metadata without touching the pixels.
        case strip = 2
    }

    /// What became of a high dynamic range photograph's gain map.
    enum Hdr: Int32 {
        case absent = 0
        case preserved = 1
        case dropped = 2
    }

    /// What the engine is asked to write.
    ///
    /// `jpeg` is the plain case: a JPEG or PNG is re-encoded as itself, a PDF
    /// is rewritten as a PDF, and anything else becomes a JPEG. Naming `avif`
    /// is a conversion of whatever arrived, and the result is a different kind
    /// of file that goes beside the original.
    enum Format: Int32 {
        case jpeg = 0
        case avif = 1

        /// How the format is named to a person.
        var name: String {
            switch self {
            case .jpeg: return "JPEG"
            case .avif: return "AVIF"
            }
        }
    }

    struct Result {
        let data: Data
        /// Absent in fast mode, and for images too small to judge.
        let score: Double?
        let hdr: Hdr
        /// True when the colour count was reduced, which PNG does by default.
        let quantized: Bool
        let originalBytes: Int
        /// True when the output is a different kind of file from the input: a
        /// JPEG made from a HEIC, or an AVIF made from anything. Such a result
        /// must never land on the source.
        let converted: Bool

        var ratio: Double { Double(data.count) / Double(originalBytes) }

        /// The extension the bytes actually are, read from their magic rather
        /// than from what was asked for. AVIF is the only ISOBMFF file the
        /// engine writes, so a `ftyp` box at offset 4 is enough to name it.
        var outputExtension: String {
            if data.starts(with: [0x89, 0x50, 0x4E, 0x47]) { return "png" }
            if data.starts(with: Array("%PDF".utf8)) { return "pdf" }
            if data.count >= 12, data[4..<8].elementsEqual("ftyp".utf8) { return "avif" }
            return "jpg"
        }
    }

    struct Failure: LocalizedError {
        let code: Int32
        let message: String
        var errorDescription: String? { message }

        /// True when the file is simply already optimal, which is an outcome
        /// rather than a fault and should not be presented as an error.
        var isAlreadyOptimal: Bool { code == SQUINT_ERR_NO_SMALLER }
    }

    static func optimize(
        _ input: Data,
        mode: Mode,
        format: Format = .jpeg,
        target: Double = 80,
        fixedQuality: Float = 75,
        pngMinQuality: Int32 = 70,
        maxDimension: Int32? = nil
    ) throws -> Result {
        var result = input.withUnsafeBytes { raw -> SquintResult in
            let base = raw.bindMemory(to: UInt8.self).baseAddress
            return squint_optimize_as(
                base, input.count, format.rawValue, mode.rawValue, target, fixedQuality,
                pngMinQuality, maxDimension ?? 0
            )
        }
        defer { squint_result_free(result) }

        guard result.error == SQUINT_OK, let bytes = result.data else {
            let message: String
            if let msgPtr = result.error_message, msgPtr.pointee != 0 {
                message = String(cString: msgPtr)
            } else {
                message = String(cString: squint_error_message(result.error))
            }
            throw Failure(code: result.error, message: message)
        }

        // Copy before the deferred free reclaims the Rust allocation.
        let data = Data(bytes: bytes, count: result.len)
        return Result(
            data: data,
            score: result.score.isNaN ? nil : result.score,
            hdr: Hdr(rawValue: result.hdr) ?? .absent,
            quantized: result.quantized != 0,
            originalBytes: result.original_len,
            converted: result.converted != 0
        )
    }
}
