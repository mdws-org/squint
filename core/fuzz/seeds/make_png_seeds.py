"""Writes the seed corpus for the `png` fuzz target into seeds/png/.

Without seeds the fuzzer gets past the signature but rarely builds a chunk
chain that reaches `IEND`, so `png::chunks` refuses almost every input and the
strip and colour-chunk code behind it is barely reached. Each seed is a small
PNG that libpng decodes, and each one carries chunks the module reads, keeps
or drops: the colour chunks `iCCP`, `sRGB`, `gAMA`, `cHRM` and `cICP`; the
text chunks `tEXt`, `zTXt` and `iTXt` (compressed and not); `eXIf` holding a
TIFF block in each byte order, with orientations that are read (3, 6) and
ones that are not (typed LONG); `tIME`, `pHYs`, `bKGD` and a `caBX`; and the
layouts the pixels come in: palette with `tRNS`, 16-bit, Adam7 interlaced,
`IDAT` split across chunks, and an animation of `acTL`, `fcTL` and `fdAT`.

Standard library only; the output is the same on every run. Run from any
directory with `python3 make_png_seeds.py`.
"""

import pathlib
import struct
import zlib

OUT = pathlib.Path(__file__).resolve().parent / "png"

SIGNATURE = b"\x89PNG\r\n\x1a\n"


def chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def ihdr(w: int, h: int, depth: int, colour: int, interlace: int = 0) -> bytes:
    return chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, depth, colour, 0, 0, interlace))


def png(*chunks: bytes) -> bytes:
    return SIGNATURE + b"".join(chunks) + chunk(b"IEND", b"")


def scanlines(rows: list, bpp: int) -> bytes:
    """Rows of packed samples, each led by a filter byte (0 none, 1 sub, 2 up, cycling).

    The filter is applied, not just named, so every row decodes to its samples.
    `bpp` is the bytes per pixel the sub filter steps back by, 1 below 8 bits.
    """
    out, prev = bytearray(), b""
    for i, row in enumerate(rows):
        kind = i % 3
        if kind == 1:
            row_out = bytes((row[j] - (row[j - bpp] if j >= bpp else 0)) & 0xFF for j in range(len(row)))
        elif kind == 2:
            row_out = bytes((row[j] - prev[j]) & 0xFF for j in range(len(row)))
        else:
            row_out = row
        out.append(kind)
        out += row_out
        prev = row
    return bytes(out)


def image_data(rows: list, bpp: int) -> bytes:
    return zlib.compress(scanlines(rows, bpp), 9)


def rgb8_rows(w: int, h: int) -> list:
    return [bytes(v for x in range(w) for v in ((x * 16) % 256, (y * 16) % 256, 128)) for y in range(h)]


def tiff(big: bool, orientation: int, orient_type: int = 3) -> bytes:
    """A TIFF block: IFD0 with Make, Orientation and Software, then their text."""
    e = ">" if big else "<"
    make, software = b"SeedCam\x00", b"Seed Writer 1.0\x00"
    entries = 3
    data_at = 8 + 2 + entries * 12 + 4
    out = (b"MM\x00\x2a" if big else b"II\x2a\x00") + struct.pack(e + "I", 8)
    out += struct.pack(e + "H", entries)
    out += struct.pack(e + "HHII", 0x010F, 2, len(make), data_at)
    if orient_type == 3:
        out += struct.pack(e + "HHIHH", 0x0112, 3, 1, orientation, 0)
    else:
        out += struct.pack(e + "HHII", 0x0112, orient_type, 1, orientation)
    out += struct.pack(e + "HHII", 0x0131, 2, len(software), data_at + len(make))
    out += struct.pack(e + "I", 0)
    return out + make + software


def icc_profile() -> bytes:
    """A small v4.3 display profile: sRGB primaries, a 2.2 curve, a description."""

    def mluc(text: str) -> bytes:
        s = text.encode("utf-16-be")
        return b"mluc\0\0\0\0" + struct.pack(">II2s2sII", 1, 12, b"en", b"US", len(s), 28) + s

    def xyz(x: int, y: int, z: int) -> bytes:
        return b"XYZ \0\0\0\0" + struct.pack(">3i", x, y, z)

    curve = b"para\0\0\0\0" + struct.pack(">HHi", 0, 0, 0x00023333)
    tags = [
        (b"desc", mluc("Seed display")), (b"cprt", mluc("No copyright, use freely")),
        (b"wtpt", xyz(0xF6D6, 0x10000, 0xD32D)), (b"rXYZ", xyz(0x6FA2, 0x38F5, 0x0390)),
        (b"gXYZ", xyz(0x6299, 0xB785, 0x18DA)), (b"bXYZ", xyz(0x24A0, 0x0F84, 0xB6CF)),
        (b"rTRC", curve), (b"gTRC", curve), (b"bTRC", curve),
    ]
    table, data, at = b"", b"", 128 + 4 + 12 * len(tags)
    for sig, body in tags:
        table += struct.pack(">4sII", sig, at + len(data), len(body))
        data += body + b"\0" * (-len(body) % 4)
    size = at + len(data)
    header = struct.pack(
        ">I4sI4s4s4s6H4sI4sIIQI3i4s16s28x",
        size, b"\x00\x00\x00\x00", 0x04300000, b"mntr", b"RGB ", b"XYZ ",
        2026, 1, 1, 0, 0, 0, b"acsp", 0, b"\x00\x00\x00\x00", 0, 0, 0, 0,
        0x0000F6D6, 0x00010000, 0x0000D32D, b"\x00\x00\x00\x00", bytes(16),
    )
    return header + struct.pack(">I", len(tags)) + table + data


TIME = chunk(b"tIME", struct.pack(">HBBBBB", 2026, 1, 2, 12, 0, 0))
PHYS = chunk(b"pHYs", struct.pack(">IIB", 2835, 2835, 1))


def rgb8_text() -> bytes:
    """Truecolour with sRGB, gAMA and cHRM, and every kind of text chunk."""
    w = h = 8
    return png(
        ihdr(w, h, 8, 2),
        chunk(b"sRGB", b"\x00"),
        chunk(b"gAMA", struct.pack(">I", 45455)),
        chunk(b"cHRM", struct.pack(">8I", 31270, 32900, 64000, 33000, 30000, 60000, 15000, 6000)),
        PHYS,
        chunk(b"tEXt", b"Software\x00Seed Writer 1.0"),
        chunk(b"zTXt", b"Comment\x00\x00" + zlib.compress(b"a prompt and a seed", 9)),
        chunk(b"iTXt", b"Title\x00\x00\x00en\x00Titel\x00Seed"),
        chunk(b"IDAT", image_data(rgb8_rows(w, h), 3)),
        chunk(b"iTXt", b"Description\x00\x01\x00\x00\x00" + zlib.compress("café".encode(), 9)),
        TIME,
    )


def palette_trns() -> bytes:
    """Four-bit palette with tRNS, an iCCP profile, and eXIf little-endian, orientation 6."""
    w = h = 8
    plte = bytes(v for i in range(16) for v in (i * 16, 255 - i * 16, (i * 37) % 256))
    trns = bytes([0, 64, 128, 192] + [255] * 12)
    rows = [bytes(((2 * x + y) % 16) << 4 | ((2 * x + 1 + y) % 16) for x in range(w // 2)) for y in range(h)]
    return png(
        ihdr(w, h, 4, 3),
        chunk(b"iCCP", b"Seed profile\x00\x00" + zlib.compress(icc_profile(), 9)),
        chunk(b"PLTE", plte),
        chunk(b"tRNS", trns),
        chunk(b"bKGD", b"\x03"),
        chunk(b"eXIf", tiff(big=False, orientation=6)),
        chunk(b"IDAT", image_data(rows, 1)),
    )


def adam7(pixels: list, w: int, h: int) -> list:
    """Split rows of per-pixel sample bytes into the seven Adam7 passes."""
    passes = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]
    out = []
    for x0, y0, dx, dy in passes:
        rows = [b"".join(pixels[y][x] for x in range(x0, w, dx)) for y in range(y0, h, dy)]
        out.append([r for r in rows if r])
    return out


def rgba16_interlaced() -> bytes:
    """Sixteen-bit RGBA, Adam7, with cICP and eXIf big-endian, orientation 3."""
    w = h = 8
    pixels = [[struct.pack(">4H", x * 8000, y * 8000, 32768, 65535 - x * y * 1000) for x in range(w)] for y in range(h)]
    stream = b""
    for rows in adam7(pixels, w, h):
        if rows:
            stream += scanlines(rows, 8)
    return png(
        ihdr(w, h, 16, 6, interlace=1),
        chunk(b"cICP", bytes([9, 16, 0, 1])),
        chunk(b"eXIf", tiff(big=True, orientation=3)),
        chunk(b"IDAT", zlib.compress(stream, 9)),
        chunk(b"tEXt", b"Author\x00A. Writer"),
    )


def grey16_split() -> bytes:
    """Sixteen-bit greyscale, IDAT split three ways, gAMA, and a caBX before IEND."""
    w = h = 8
    rows = [b"".join(struct.pack(">H", (x * 4096 + y * 512) & 0xFFFF) for x in range(w)) for y in range(h)]
    data = image_data(rows, 2)
    third = len(data) // 3
    return png(
        ihdr(w, h, 16, 0),
        chunk(b"gAMA", struct.pack(">I", 45455)),
        chunk(b"IDAT", data[:third]),
        chunk(b"IDAT", data[third:2 * third]),
        chunk(b"IDAT", data[2 * third:]),
        chunk(b"caBX", b"\x00\x00\x00\x10jumbc2pa seed"),
    )


def animated() -> bytes:
    """An APNG: the default image is frame one, then a 4x4 frame at (2, 2).

    Its eXIf holds an orientation typed LONG, which is no orientation at all.
    """
    w = h = 8

    def fctl(seq: int, fw: int, fh: int, x: int, y: int, dispose: int, blend: int) -> bytes:
        return chunk(b"fcTL", struct.pack(">IIIIIHHBB", seq, fw, fh, x, y, 1, 10, dispose, blend))

    first = image_data(rgb8_rows(w, h), 3)
    small = image_data([bytes(v for _ in range(4) for v in (255, 0, 0)) for _ in range(4)], 3)
    return png(
        ihdr(w, h, 8, 2),
        chunk(b"acTL", struct.pack(">II", 2, 0)),
        chunk(b"eXIf", tiff(big=False, orientation=6, orient_type=4)),
        fctl(0, w, h, 0, 0, 0, 0),
        chunk(b"IDAT", first),
        fctl(1, 4, 4, 2, 2, 1, 1),
        chunk(b"fdAT", struct.pack(">I", 2) + small),
    )


SEEDS = {
    "rgb8-text.png": rgb8_text,
    "palette-trns-iccp.png": palette_trns,
    "rgba16-interlaced.png": rgba16_interlaced,
    "grey16-split-idat.png": grey16_split,
    "animated.png": animated,
}


def main() -> None:
    OUT.mkdir(exist_ok=True)
    for name, build in SEEDS.items():
        data = build()
        (OUT / name).write_bytes(data)
        print(f"{name}: {len(data)} bytes")


if __name__ == "__main__":
    main()
