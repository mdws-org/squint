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

    struct Result {
        let data: Data
        /// Absent in fast mode, and for images too small to judge.
        let score: Double?
        let hdr: Hdr
        /// True when the colour count was reduced, which PNG does by default.
        let quantized: Bool
        let originalBytes: Int

        var ratio: Double { Double(data.count) / Double(originalBytes) }
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
        target: Double = 80,
        fixedQuality: Float = 75,
        pngMinQuality: Int32 = 70
    ) throws -> Result {
        var result = input.withUnsafeBytes { raw -> SquintResult in
            let base = raw.bindMemory(to: UInt8.self).baseAddress
            return squint_optimize(base, input.count, mode.rawValue, target, fixedQuality, pngMinQuality)
        }
        defer { squint_result_free(result) }

        guard result.error == SQUINT_OK, let bytes = result.data else {
            let message = String(cString: squint_error_message(result.error))
            throw Failure(code: result.error, message: message)
        }

        // Copy before the deferred free reclaims the Rust allocation.
        let data = Data(bytes: bytes, count: result.len)
        return Result(
            data: data,
            score: result.score.isNaN ? nil : result.score,
            hdr: Hdr(rawValue: result.hdr) ?? .absent,
            quantized: result.quantized != 0,
            originalBytes: result.original_len
        )
    }
}
