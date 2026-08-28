import hashlib, json, os, re, struct, sys

# Malformation families. Each takes the seed bytes and returns a list of
# (suffix, bytes, note). Positions are fixed rather than random so a corpus
# regenerates byte-identically from the same seed.

FRACTIONS = [0.10, 0.35, 0.60, 0.85, 0.99]


def jpeg_segments(d):
    # Offsets of each marker segment header, up to the start of scan.
    out, i = [], 2
    while i + 4 <= len(d) and d[i] == 0xFF:
        m = d[i + 1]
        if m in (0xD8, 0x01) or 0xD0 <= m <= 0xD7:
            i += 2
            continue
        if m in (0xDA, 0xD9):
            out.append((i, m, 0))
            break
        ln = int.from_bytes(d[i + 2:i + 4], "big")
        out.append((i, m, ln))
        i += 2 + ln
    return out


def jpeg_variants(d):
    v = []
    for f in FRACTIONS:
        n = max(2, int(len(d) * f))
        v.append(("trunc-%02d" % int(f * 100), d[:n],
                  "truncated to %d%% of %d bytes" % (int(f * 100), len(d))))

    segs = [s for s in jpeg_segments(d) if s[2] > 0]
    for idx, (off, m, ln) in enumerate(segs[:6]):
        # A length wrong by one byte desyncs the walk. This is the exact shape
        # of the defect the 2026-08-19 corpus caught, where a desynced walk
        # produced a headers-only file that passed the never-grow check and was
        # written over the photograph.
        for delta, tag in ((1, "plus1"), (-1, "minus1")):
            b = bytearray(d)
            b[off + 2:off + 4] = struct.pack(">H", max(2, ln + delta))
            v.append(("seglen-%s-FF%02X-%d" % (tag, m, idx), bytes(b),
                      "APP%X length %d declared as %d" % (m & 0x0F, ln, ln + delta)))
        b = bytearray(d)
        b[off + 2:off + 4] = struct.pack(">H", 0xFFFF)
        v.append(("seglen-huge-FF%02X-%d" % (m, idx), bytes(b),
                  "segment length claims 65535, past end of file"))

    sos = [s for s in jpeg_segments(d) if s[1] == 0xDA]
    if sos:
        off = sos[0][0]
        v.append(("no-sos", d[:off] + d[off + 2:],
                  "start-of-scan marker removed"))
        for k, pos in enumerate((off + 64, off + 512, (off + len(d)) // 2)):
            if pos < len(d):
                b = bytearray(d)
                b[pos] ^= 0x40
                v.append(("bitflip-entropy-%d" % k, bytes(b),
                          "bit flipped at offset %d, inside entropy-coded data" % pos))

    if d.endswith(b"\xff\xd9"):
        v.append(("no-eoi", d[:-2], "end-of-image marker removed"))
    return v


def png_chunks(d):
    out, i = [], 8
    while i + 8 <= len(d):
        ln = int.from_bytes(d[i:i + 4], "big")
        typ = d[i + 4:i + 8]
        out.append((i, typ, ln))
        if typ == b"IEND":
            break
        i += 12 + ln
    return out


def png_variants(d):
    v = []
    for f in FRACTIONS:
        n = max(8, int(len(d) * f))
        v.append(("trunc-%02d" % int(f * 100), d[:n],
                  "truncated to %d%% of %d bytes" % (int(f * 100), len(d))))

    for idx, (off, typ, ln) in enumerate(png_chunks(d)[:6]):
        name = typ.decode("ascii", "replace")
        for delta, tag in ((1, "plus1"), (-1, "minus1")):
            b = bytearray(d)
            b[off:off + 4] = struct.pack(">I", max(0, ln + delta))
            v.append(("chunklen-%s-%s-%d" % (tag, name, idx), bytes(b),
                      "%s length %d declared as %d" % (name, ln, ln + delta)))
        b = bytearray(d)
        b[off:off + 4] = struct.pack(">I", 0x7FFFFFFF)
        v.append(("chunklen-huge-%s-%d" % (name, idx), bytes(b),
                  "%s length claims 2GiB" % name))
        crc = off + 8 + ln
        if crc + 4 <= len(d):
            b = bytearray(d)
            b[crc] ^= 0xFF
            v.append(("bad-crc-%s-%d" % (name, idx), bytes(b),
                      "%s CRC corrupted" % name))

    idat = [c for c in png_chunks(d) if c[1] == b"IDAT"]
    if idat:
        off, _, ln = idat[0]
        for k, pos in enumerate((off + 8, off + 8 + ln // 2)):
            if pos < len(d):
                b = bytearray(d)
                b[pos] ^= 0x40
                v.append(("bitflip-idat-%d" % k, bytes(b),
                          "bit flipped at offset %d, inside IDAT" % pos))
    iend = [c for c in png_chunks(d) if c[1] == b"IEND"]
    if iend:
        v.append(("no-iend", d[:iend[0][0]], "IEND chunk removed"))
    return v


def main(argv):
    if len(argv) < 2:
        print("usage: python3 tools/corpus-gen.py SEED [SEED ...] [-o OUTDIR]")
        print("")
        print("Writes malformed variants of each seed image plus manifest.json.")
        print("Seeds must be real JPEG or PNG files; the malformations are")
        print("structural, so a synthetic seed would not exercise the same paths.")
        return 2

    out = "corpus"
    args = []
    i = 0
    while i < len(argv[1:]):
        a = argv[1 + i]
        if a == "-o":
            out = argv[2 + i]
            i += 2
        else:
            args.append(a)
            i += 1

    os.makedirs(out, exist_ok=True)
    manifest = []
    for seed in args:
        d = open(seed, "rb").read()
        base = os.path.basename(seed)
        stem, ext = os.path.splitext(base)
        if d[:2] == b"\xff\xd8":
            variants, kind = jpeg_variants(d), "jpeg"
        elif d[:8] == b"\x89PNG\r\n\x1a\n":
            variants, kind = png_variants(d), "png"
        else:
            print("skipping %s: not a JPEG or PNG" % seed)
            continue

        for suffix, blob, note in variants:
            name = "%s-%s%s" % (stem, suffix, ext)
            open(os.path.join(out, name), "wb").write(blob)
            manifest.append({
                "file": name,
                "kind": kind,
                "family": re.sub(r"-\d+$", "", suffix),
                "seed": base,
                "seed_sha256": hashlib.sha256(d).hexdigest(),
                "sha256": hashlib.sha256(blob).hexdigest(),
                "bytes": len(blob),
                "note": note,
            })
        print("%s: %d variants" % (base, len(variants)))

    open(os.path.join(out, "manifest.json"), "w").write(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print("%d files in %s/" % (len(manifest), out))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
