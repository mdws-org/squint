"""Writes the seed corpus for the `webp` fuzz target into seeds/webp/.

Without seeds the fuzzer rarely produces a RIFF header whose size field
matches the bytes present, so `webp::walk` refuses almost every input and the
chunk handling behind it is barely reached. Each seed is a small WebP that
libwebp decodes, and each one exercises part of what the module reads: a
lossy `VP8 ` frame, a lossless `VP8L` frame, a `VP8X` header announcing
`ICCP`, `EXIF` and `XMP `, a raw `ALPH` plane, an odd-length chunk with its
pad byte, and an animation of `ANMF` frames. The `EXIF` chunks hold TIFF
blocks in both byte orders, with and without the `Exif\\0\\0` marker, and with
orientations that are read (6, 8) and ones that are not (typed LONG, value 9).

Standard library only; the output is the same on every run. Run from any
directory with `python3 make_webp_seeds.py`.
"""

import pathlib
import struct

OUT = pathlib.Path(__file__).resolve().parent / "webp"

# A 16x16 lossy key frame: the `VP8 ` payload libwebp's cwebp 1.6.0 wrote for
# a synthetic gradient (red rising left to right, green top to bottom, blue
# 128) at -q 40 -m 6 -sns 0 -f 0. A VP8 frame needs the boolean entropy coder
# and the 1,056 coefficient update probabilities of RFC 6386 to write by hand,
# so these 58 bytes stand in for an encoder.
VP8_16X16 = bytes.fromhex(
    "b401009d012a1000100000005a0009d1fc00605145e000fef49c64898f09bfa4"
    "633b6d88a4fc9e26f1d259d261a6b0cdfce5d07f1f2479240000"
)

W, H = 16, 16


def riff(chunks: list) -> bytes:
    """A WebP from (FourCC, payload) pairs, pad bytes and size field computed."""
    body = bytearray(b"WEBP")
    for fourcc, payload in chunks:
        body += fourcc + struct.pack("<I", len(payload)) + payload
        if len(payload) & 1:
            body += b"\x00"
    return b"RIFF" + struct.pack("<I", len(body)) + bytes(body)


def u24(n: int) -> bytes:
    return struct.pack("<I", n)[:3]


def vp8x(flags: int, w: int = W, h: int = H) -> bytes:
    return bytes([flags, 0, 0, 0]) + u24(w - 1) + u24(h - 1)


ICC_FLAG, ALPHA_FLAG, EXIF_FLAG, XMP_FLAG, ANIM_FLAG = 0x20, 0x10, 0x08, 0x04, 0x02


class Bits:
    """The VP8L bit writer: least significant bit first."""

    def __init__(self) -> None:
        self.value, self.count = 0, 0

    def put(self, value: int, n: int) -> None:
        self.value |= value << self.count
        self.count += n

    def bytes(self) -> bytes:
        return self.value.to_bytes((self.count + 7) // 8, "little")


def vp8l(w: int, h: int, green: tuple, red: int, blue: int, alpha: tuple, pattern) -> bytes:
    """A lossless frame built from simple prefix codes, no transforms.

    `green` and `alpha` each hold one or two symbols. One symbol costs no bits
    per pixel; two cost one bit, 0 for the smaller symbol and 1 for the larger,
    chosen per pixel by `pattern(x, y)` (green) and its inverse (alpha).
    """
    b = Bits()
    b.put(0x2F, 8)
    b.put(w - 1, 14)
    b.put(h - 1, 14)
    b.put(1 if alpha != (255,) else 0, 1)  # alpha_is_used
    b.put(0, 3)  # version
    b.put(0, 1)  # no transform
    b.put(0, 1)  # no colour cache
    b.put(0, 1)  # no meta prefix codes

    def simple(symbols: tuple) -> None:
        b.put(1, 1)  # simple code
        b.put(len(symbols) - 1, 1)
        b.put(1, 1)  # first symbol takes eight bits
        b.put(symbols[0], 8)
        if len(symbols) == 2:
            b.put(symbols[1], 8)

    simple(green)
    simple((red,))
    simple((blue,))
    simple(alpha)
    simple((0,))  # distance: never used, no back-references
    green_lo, alpha_lo = min(green), min(alpha)
    for y in range(h):
        for x in range(w):
            on = pattern(x, y)
            if len(green) == 2:
                g = green[on]
                b.put(0 if g == green_lo else 1, 1)
            if len(alpha) == 2:
                a = alpha[1 - on]
                b.put(0 if a == alpha_lo else 1, 1)
    return b.bytes()


def tiff(big: bool, orientation: int, orient_type: int = 3, marker: bool = False) -> bytes:
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
    out += make + software
    return (b"Exif\x00\x00" if marker else b"") + out


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


# Odd length on purpose: the chunk after it starts one pad byte later.
XMP_PACKET = (
    b'<?xpacket begin="\xef\xbb\xbf" id="W5M0MpCehiHzreSzNTczkc9d"?>'
    b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
    b'<rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:CreatorTool="Seed Writer"/>'
    b'</rdf:RDF></x:xmpmeta><?xpacket end="w"?>'
)
assert len(XMP_PACKET) % 2 == 1


def checker(x: int, y: int) -> int:
    return ((x >> 2) + (y >> 2)) & 1


def lossy() -> bytes:
    return riff([(b"VP8 ", VP8_16X16)])


def lossless() -> bytes:
    return riff([(b"VP8L", vp8l(W, H, (40, 200), 90, 160, (255,), checker))])


def extended_metadata() -> bytes:
    """VP8X announcing ICCP, EXIF and XMP, in the order the format lays them out."""
    return riff([
        (b"VP8X", vp8x(ICC_FLAG | EXIF_FLAG | XMP_FLAG)),
        (b"ICCP", icc_profile()),
        (b"VP8 ", VP8_16X16),
        (b"EXIF", tiff(big=False, orientation=6)),
        (b"XMP ", XMP_PACKET),
    ])


def extended_alpha() -> bytes:
    """A raw ALPH plane before the lossy frame; EXIF big-endian behind Exif\\0\\0."""
    alpha = bytes(255 if checker(x, y) else 64 for y in range(H) for x in range(W))
    return riff([
        (b"VP8X", vp8x(ALPHA_FLAG | EXIF_FLAG | XMP_FLAG)),
        (b"ALPH", b"\x00" + alpha),
        (b"VP8 ", VP8_16X16),
        (b"XMP ", XMP_PACKET),
        (b"EXIF", tiff(big=True, orientation=8, marker=True)),
    ])


def lossless_alpha_icc() -> bytes:
    """A lossless frame with two alpha levels and a profile; orientation typed LONG."""
    return riff([
        (b"VP8X", vp8x(ICC_FLAG | ALPHA_FLAG | EXIF_FLAG)),
        (b"ICCP", icc_profile()),
        (b"VP8L", vp8l(W, H, (10, 250), 120, 30, (128, 255), checker)),
        (b"EXIF", tiff(big=True, orientation=6, orient_type=4)),
    ])


def anmf(x: int, y: int, w: int, h: int, duration: int, flags: int, frame: list) -> bytes:
    payload = u24(x // 2) + u24(y // 2) + u24(w - 1) + u24(h - 1) + u24(duration) + bytes([flags])
    for fourcc, data in frame:
        payload += fourcc + struct.pack("<I", len(data)) + data + (b"\x00" if len(data) & 1 else b"")
    return payload


def animated() -> bytes:
    """Two frames: a full-canvas lossy frame, then a lossless 8x8 blended at (8, 8)."""
    small = vp8l(8, 8, (0, 255), 255, 0, (0, 255), lambda x, y: (x + y) & 1)
    return riff([
        (b"VP8X", vp8x(ANIM_FLAG | ALPHA_FLAG | EXIF_FLAG)),
        (b"ANIM", struct.pack("<IH", 0xFFFFFFFF, 0)),
        (b"ANMF", anmf(0, 0, W, H, 100, 0b10, [(b"VP8 ", VP8_16X16)])),
        (b"ANMF", anmf(8, 8, 8, 8, 100, 0b01, [(b"VP8L", small)])),
        (b"EXIF", tiff(big=False, orientation=9)),
    ])


SEEDS = {
    "lossy.webp": lossy,
    "lossless.webp": lossless,
    "extended-metadata.webp": extended_metadata,
    "extended-alpha.webp": extended_alpha,
    "lossless-alpha-icc.webp": lossless_alpha_icc,
    "animated.webp": animated,
}


def main() -> None:
    OUT.mkdir(exist_ok=True)
    for name, build in SEEDS.items():
        data = build()
        (OUT / name).write_bytes(data)
        print(f"{name}: {len(data)} bytes")


if __name__ == "__main__":
    main()
