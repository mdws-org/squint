// C interface to the squint engine.
//
// Every buffer returned by squint_optimize was allocated by Rust. Pass the whole
// result to squint_result_free exactly once. Do not free the pointer directly.

#ifndef SQUINT_H
#define SQUINT_H

#include <stddef.h>
#include <stdint.h>

#define SQUINT_OK             0
#define SQUINT_ERR_DECODE     1
#define SQUINT_ERR_ENCODE     2
#define SQUINT_ERR_METRIC     3
#define SQUINT_ERR_TOO_SMALL  4
#define SQUINT_ERR_UNREACHABLE 5
#define SQUINT_ERR_NO_SMALLER 6
#define SQUINT_ERR_NULL_INPUT 7
#define SQUINT_ERR_TOO_LARGE  8
#define SQUINT_ERR_PANIC      9
#define SQUINT_ERR_COLOURS    10
#define SQUINT_ERR_READ_ONLY  11
#define SQUINT_ERR_UNKNOWN_FORMAT 12
#define SQUINT_ERR_UNKNOWN_MODE   13

#define SQUINT_MODE_FAST    0
#define SQUINT_MODE_QUALITY 1
#define SQUINT_MODE_STRIP   2

// What squint_optimize_as writes. JPEG is what squint_optimize asks for: a JPEG
// or PNG is re-encoded as itself, a PDF is rewritten as a PDF, and everything
// else becomes a JPEG. AVIF and lossless WebP are conversions of any picture.
#define SQUINT_FORMAT_JPEG          0
#define SQUINT_FORMAT_AVIF          1
#define SQUINT_FORMAT_WEBP_LOSSLESS 2

// What became of a high dynamic range gain map.
#define SQUINT_HDR_ABSENT    0
#define SQUINT_HDR_PRESERVED 1
#define SQUINT_HDR_DROPPED   2

typedef struct {
    uint8_t *data;
    size_t   len;
    size_t   original_len;
    double   score;   // NaN when no metric was evaluated
    int      hdr;     // one of the SQUINT_HDR_ values
    int      quantized; // non-zero when the colour count was reduced
    int      error;   // SQUINT_OK, or one of the SQUINT_ERR_ values
    // This failure's own sentence, carrying whatever numbers it measured, or
    // null when there is none. Owned by Rust and released by squint_result_free
    // along with the data buffer. When it is null, squint_error_message(error)
    // is the description to show.
    const char *error_message;
    // Non-zero when the output is a different kind of file from the input: a
    // JPEG made from a HEIC, WebP or SVG, or an AVIF made from anything. Such a
    // result must be written beside the original, never over it.
    int converted;
} SquintResult;

// The input format is detected from the bytes. HEIC is decoded for fast and
// quality modes and comes back as JPEG. mode is one of the SQUINT_MODE_ values
// and any other is refused with SQUINT_ERR_UNKNOWN_MODE. png_min_quality below
// 0 disables quantization. max_dimension caps the long edge in pixels; 0
// leaves the picture its own size. The cap never enlarges.
SquintResult squint_optimize(const uint8_t *input, size_t input_len, int mode,
                             double target, float fixed_quality, int png_min_quality,
                             int max_dimension);

// squint_optimize with the output format named: one of the SQUINT_FORMAT_
// values. Any other value is refused with SQUINT_ERR_UNKNOWN_FORMAT rather than
// read as JPEG. A PDF is rewritten as a PDF only when JPEG is named, and refused
// with SQUINT_ERR_READ_ONLY otherwise.
SquintResult squint_optimize_as(const uint8_t *input, size_t input_len, int format,
                                int mode, double target, float fixed_quality,
                                int png_min_quality, int max_dimension);

void squint_result_free(SquintResult result);

// Static string, never null, never freed.
const char *squint_error_message(int code);

#endif
