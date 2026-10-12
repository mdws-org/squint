#!/opt/homebrew/bin/python3
"""Write option-C variants of a Squint-stripped HEIC, using the original to find the item extents.

usage: make_variants.py ORIGINAL STRIPPED OUTDIR
Writes: c_exif.heic (valid empty TIFF in the Exif item, XMP left zero-filled as squint does today),
        c_all.heic (valid empty TIFF + a valid empty XMP packet padded with spaces in every mime item).
"""
import struct, sys
orig, stripped, out = sys.argv[1], sys.argv[2], sys.argv[3]
b = open(orig, "rb").read(); s = bytearray(open(stripped, "rb").read())

def boxes(lo, hi):
    at = lo
    while at + 8 <= hi:
        sz, t = struct.unpack(">I4s", b[at:at + 8]); hdr = 8
        if sz == 1: sz = struct.unpack(">Q", b[at + 8:at + 16])[0]; hdr = 16
        yield t, at, at + sz, at + hdr
        at += sz

meta = next((h, e) for t, _, e, h in boxes(0, len(b)) if t == b"meta")
sub = {t: (h, e) for t, _, e, h in boxes(meta[0] + 4, meta[1])}
h, e = sub[b"iinf"]; ver = b[h]; at = h + (6 if ver == 0 else 8); types = {}
while at < e:
    sz = struct.unpack(">I", b[at:at + 4])[0]
    types[struct.unpack(">H", b[at + 12:at + 14])[0]] = b[at + 16:at + 20]; at += sz
h, e = sub[b"iloc"]; v = b[h]; at = h + 4
osz, lsz, bsz, isz = b[at] >> 4, b[at] & 15, b[at + 1] >> 4, b[at + 1] & 15; at += 2
def rd(w):
    global at
    x = int.from_bytes(b[at:at + w], "big") if w else 0; at += w; return x
n = rd(2 if v < 2 else 4); ext = {}
for _ in range(n):
    iid = rd(2 if v < 2 else 4); m = rd(2) & 15 if v >= 1 else 0; rd(2); base = rd(bsz); cnt = rd(2); xs = []
    for _ in range(cnt):
        if v >= 1 and isz: rd(isz)
        o = rd(osz); l = rd(lsz); xs.append((base + o, l))
    if m == 0: ext[iid] = xs

EXIF = bytes.fromhex("00000000 4D4D002A 00000008 0000 00000000".replace(" ", ""))
HEAD = '<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"/>'.encode()
TAIL = b'<?xpacket end="w"?>'

def write(path, do_xmp):
    o = bytearray(s)
    for iid, t in types.items():
        if t not in (b"Exif", b"mime") or iid not in ext: continue
        (start, length), = ext[iid]
        if t == b"Exif":
            o[start:start + length] = EXIF + bytes(length - len(EXIF))
        elif do_xmp:
            o[start:start + length] = HEAD + b" " * (length - len(HEAD) - len(TAIL)) + TAIL
        print(path, "item", iid, t.decode(), "len", length)
    open(path, "wb").write(o)

write(out + "/c_exif.heic", False)
write(out + "/c_all.heic", True)
