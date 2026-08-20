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

#define SQUINT_MODE_FAST    0
#define SQUINT_MODE_QUALITY 1
#define SQUINT_MODE_STRIP   2

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
} SquintResult;

// Format is detected from the bytes. png_min_quality below 0 disables quantization.
SquintResult squint_optimize(const uint8_t *input, size_t input_len, int mode,
                             double target, float fixed_quality, int png_min_quality);

void squint_result_free(SquintResult result);

// Static string, never null, never freed.
const char *squint_error_message(int code);

#endif
