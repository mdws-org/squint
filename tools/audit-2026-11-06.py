#!/opt/homebrew/bin/python3
"""Repro files for the 2026-11-06 audit, and the outside checks that judge them.

    audit-2026-11-06.py make DIR SQUINT
    audit-2026-11-06.py judge DIR SQUINT
    audit-2026-11-06.py measure DIR SQUINT

`make` writes one file per finding into DIR. Where a real writer can produce
the shape, it does: `sips` writes every base picture and `exiftool` writes the
orientation into the WebP and PNG and the comment into the JPEG. The shapes no
installed tool writes (a GIF extension carrying extra sub-blocks, a HEIF box
or item name, a PDF with scripts, a LONG orientation, a stereo index) are
built from those real files by the functions below. SQUINT is used only to
write the base WebP, since nothing else installed writes one.

`judge` runs SQUINT over each file and checks the result with tools that are
not squint: `exiftool -a -G1 -u`, Image I/O through `imageio-probe.swift`, and
a byte scan of the output (with every Flate stream inflated, for a PDF). It
prints one line per check and exits 1 when any check finds the defect.

`measure` reports the peak memory of a quality search at three picture sizes,
from `/usr/bin/time -l`. It needs a few gigabytes free.

macOS only: `sips`, `swift` and `/usr/bin/time -l` are.
"""

import hashlib
import os
import re
import shutil
import struct
import subprocess
import sys
import time
import zlib

EXIFTOOL = shutil.which("exiftool") or "/opt/homebrew/bin/exiftool"
HERE = os.path.dirname(os.path.abspath(__file__))
PROBE = os.path.join(HERE, "imageio-probe.swift")

# Strings planted in the repro files. Finding one in an output means the
# thing that carried it survived.
PERSON = b"uid=jdoe GPS 51.5007,-0.1246"
XMP_MARK = b"GPS 33.4484,-112.0740"
ITEM_NAME = b"IMG_0042.HEIC"
KEYS_MARK = b"Jane Q Roofer"
AUTHOR = b"Jane Doe"
DATE = b"D:20260101120000"


def run(args, **kw):
    return subprocess.run(args, capture_output=True, **kw)


def write(path, data):
    with open(path, "wb") as f:
        f.write(data)


def read(path):
    with open(path, "rb") as f:
        return f.read()


# ---------------------------------------------------------------- pictures

def png_bytes(width, height, rgb):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        raw.extend(rgb[y * width * 3:(y + 1) * width * 3])
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(raw), 6))
            + chunk(b"IEND", b""))


def gradient(width, height, seed):
    out = bytearray()
    for y in range(height):
        for x in range(width):
            out += bytes(((x * 4 + seed) % 256, (y * 5) % 256, (x + y + seed) % 256))
    return bytes(out)


def bmp_noise(path, width, height):
    """A 24-bit BMP of fixed noise, written a row at a time."""
    row = (width * 3 + 3) & ~3
    header = b"BM" + struct.pack("<IHHI", 54 + row * height, 0, 0, 54)
    info = struct.pack("<IiiHHIIiiII", 40, width, height, 1, 24, 0, row * height, 2835, 2835, 0, 0)
    state = 12345
    with open(path, "wb") as f:
        f.write(header + info)
        for _ in range(height):
            state = (state * 1103515245 + 12345) & 0x7FFFFFFF
            line = hashlib.shake_128(state.to_bytes(4, "little")).digest(width * 3)
            f.write(line + b"\0" * (row - width * 3))


def sips(src, fmt, out, *extra):
    r = run(["sips", "-s", "format", fmt, *extra, src, "--out", out])
    if r.returncode != 0 or not os.path.exists(out):
        sys.exit(f"sips could not write {out}: {r.stderr.decode(errors='replace')}")


def exiftool_write(path, *args):
    r = run([EXIFTOOL, "-q", "-overwrite_original", *args, path])
    if r.returncode != 0:
        sys.exit(f"exiftool could not write {path}: {r.stderr.decode(errors='replace')}")


# ---------------------------------------------------------------- GIF

def gif_blocks_start(gif):
    gct = 3 * (1 << ((gif[10] & 7) + 1)) if gif[10] & 0x80 else 0
    return 13 + gct


def first_image(gif):
    """Offset of the first image descriptor, walking the blocks before it."""
    pos = gif_blocks_start(gif)
    while gif[pos] == 0x21:
        pos += 2
        while gif[pos]:
            pos += 1 + gif[pos]
        pos += 1
    if gif[pos] != 0x2C:
        sys.exit("the sips GIF is not laid out as expected")
    return pos


def gif_extension(label, blocks):
    out = bytearray([0x21, label])
    for b in blocks:
        out += bytes([len(b)]) + b
    return bytes(out + b"\0")


def make_gifs(d):
    sips(os.path.join(d, "base.png"), "gif", os.path.join(d, "base.gif"))
    gif = read(os.path.join(d, "base.gif"))
    at = first_image(gif)
    # A Graphic Control Extension is one four-byte sub-block. This one carries
    # a second sub-block after it.
    # A comment, which Strip removes, so the run has something to report.
    comment = gif_extension(0xFE, [b"a comment"])
    gce = gif_extension(0xF9, [b"\x00\x00\x00\x00", PERSON])
    write(os.path.join(d, "f1-gce.gif"), gif[:at] + comment + gce + gif[at:])
    # A Plain Text Extension whose grid holds one 8x8 cell, followed by text
    # far longer than one character.
    header = struct.pack("<HHHHBBBB", 0, 0, 8, 8, 8, 8, 1, 0)
    text = gif_extension(0x01, [header, PERSON])
    write(os.path.join(d, "f1-plaintext.gif"), gif[:at] + comment + text + gif[at:])


# ---------------------------------------------------------------- HEIF

def boxes(data, start, end):
    out = []
    i = start
    while i + 8 <= end:
        size, kind = struct.unpack(">I4s", data[i:i + 8])
        header = 8
        if size == 1:
            size = struct.unpack(">Q", data[i + 8:i + 16])[0]
            header = 16
        elif size == 0:
            size = end - i
        out.append((kind, i, size, header))
        i += size
    return out


def find_box(data, kind, start, end):
    for k, at, size, header in boxes(data, start, end):
        if k == kind:
            return at, size, header
    sys.exit(f"no {kind!r} box in the sips HEIC")


def shift_iloc(data, iloc_at, past, delta):
    """Move every absolute extent at or past `past` by `delta`. Handles the
    version 0 and 1 layouts `sips` writes."""
    body = iloc_at + 12
    version = data[iloc_at + 8]
    off_size, len_size = data[body] >> 4, data[body] & 15
    base_size, index_size = data[body + 1] >> 4, data[body + 1] & 15
    count = struct.unpack(">H", data[body + 2:body + 4])[0]
    i = body + 4

    def field(width, pos):
        return int.from_bytes(data[pos:pos + width], "big") if width else 0

    for _ in range(count):
        i += 2  # item id
        method = 0
        if version >= 1:
            method = struct.unpack(">H", data[i:i + 2])[0] & 15
            i += 2
        i += 2  # data reference index
        base_at, base = i, field(base_size, i)
        i += base_size
        extents = struct.unpack(">H", data[i:i + 2])[0]
        i += 2
        moved_base = method == 0 and base_size and base >= past
        if moved_base:
            data[base_at:base_at + base_size] = (base + delta).to_bytes(base_size, "big")
        for _ in range(extents):
            i += index_size if version >= 1 else 0
            offset = field(off_size, i)
            if method == 0 and not base_size and offset >= past:
                data[i:i + off_size] = (offset + delta).to_bytes(off_size, "big")
            i += off_size + len_size


def heif_insert(heic, at, payload, parents):
    """Insert `payload` at `at`, growing each enclosing box and moving the
    picture data that now sits further along."""
    data = bytearray(heic[:at] + payload + heic[at:])
    for p in parents:
        size = struct.unpack(">I", data[p:p + 4])[0]
        data[p:p + 4] = struct.pack(">I", size + len(payload))
    meta_at, meta_size, _ = find_box(data, b"meta", 0, len(data))
    iloc_at, _, _ = find_box(data, b"iloc", meta_at + 12, meta_at + meta_size)
    shift_iloc(data, iloc_at, at, len(payload))
    return bytes(data)


def bmff(kind, body):
    return struct.pack(">I", 8 + len(body)) + kind + body


def make_heifs(d):
    sips(os.path.join(d, "base.png"), "heic", os.path.join(d, "base.heic"))
    heic = read(os.path.join(d, "base.heic"))
    meta_at, meta_size, _ = find_box(heic, b"meta", 0, len(heic))
    meta_end = meta_at + meta_size

    xmp = (b"<x:xmpmeta xmlns:x='adobe:ns:meta/'><rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>"
           b"<rdf:Description xmlns:dc='http://purl.org/dc/elements/1.1/'><dc:description>"
           + XMP_MARK + b"</dc:description></rdf:Description></rdf:RDF></x:xmpmeta>")
    xml = bmff(b"xml ", b"\0\0\0\0" + xmp)
    write(os.path.join(d, "f3-xml.heic"), heif_insert(heic, meta_end, xml, [meta_at]))

    keys = bmff(b"keys", b"\0\0\0\0" + struct.pack(">I", 1) + bmff(b"mdta", b"com.apple.quicktime.author"))
    ilst = bmff(b"ilst", bmff(struct.pack(">I", 1), bmff(b"data", struct.pack(">II", 1, 0) + KEYS_MARK)))
    write(os.path.join(d, "f3-keys.heic"), heif_insert(heic, meta_end, keys + ilst, [meta_at]))

    # An item name: the null-terminated string after the item type in `infe`.
    iinf_at, iinf_size, _ = find_box(heic, b"iinf", meta_at + 12, meta_end)
    entries = iinf_at + 12 + (2 if heic[iinf_at + 8] == 0 else 4)
    infe_at, infe_size, _ = find_box(heic, b"infe", entries, iinf_at + iinf_size)
    name_at = infe_at + infe_size - 1  # the empty name's terminator
    if heic[name_at] != 0:
        sys.exit("the sips HEIC's first item already has a name")
    named = heif_insert(heic, name_at, ITEM_NAME, [meta_at, iinf_at, infe_at])
    write(os.path.join(d, "f3-name.heic"), named)


# ---------------------------------------------------------------- PDF

def pdf(objects, trailer):
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for n, body in enumerate(objects, 1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % n + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    for o in offsets:
        out += b"%010d 00000 n \n" % o
    out += b"trailer\n" + trailer % (len(objects) + 1) + b"\nstartxref\n%d\n%%%%EOF\n" % xref
    return bytes(out)


def stream(dictionary, content):
    return b"<< " + dictionary + b" /Length %d >>\nstream\n" % len(content) + content + b"\nendstream"


def make_pdf(d):
    js = b"this.info.author = \"" + AUTHOR + b"\";"
    objects = [
        # 1 catalog: a document-level script, an opening script, a script on
        # close, and a form whose only widget is reached through /Fields.
        b"<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript 5 0 R >> /OpenAction 7 0 R"
        b" /AA << /WC 8 0 R >> /AcroForm << /Fields [10 0 R] >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        # 3 page: a script when the page opens, and one annotation that
        # replies to another the page does not list.
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R"
        b" /Annots [12 0 R] /AA << /O 9 0 R >> >>",
        stream(b"", b"72 72 m 540 720 l S"),
        b"<< /Names [(init) 6 0 R] >>",
        b"<< /S /JavaScript /JS (" + js + b") >>",
        b"<< /S /JavaScript /JS (app.alert\\(\"opened\"\\);) >>",
        b"<< /S /JavaScript /JS 14 0 R >>",
        b"<< /S /JavaScript /JS (console.println\\(\"page\"\\);) >>",
        # 10 a field whose widget is its kid, absent from the page's /Annots.
        b"<< /FT /Tx /T (address) /Kids [11 0 R] >>",
        b"<< /Type /Annot /Subtype /Widget /Parent 10 0 R /P 3 0 R /Rect [72 600 300 620]"
        b" /M (" + DATE + b") /CreationDate (" + DATE + b") >>",
        b"<< /Type /Annot /Subtype /Text /P 3 0 R /Rect [72 700 92 720] /Contents (reply)"
        b" /T (" + AUTHOR + b") /M (" + DATE + b") /IRT 13 0 R >>",
        # 13 the annotation replied to, reached only through /IRT.
        b"<< /Type /Annot /Subtype /Text /P 3 0 R /Rect [100 700 120 720] /Contents (original)"
        b" /T (" + AUTHOR + b") /M (" + DATE + b") /CreationDate (" + DATE + b") >>",
        stream(b"", b"/* closed by " + AUTHOR + b" */ app.beep(0);"),
        b"<< /Author (" + AUTHOR + b") /Producer (audit fixture) >>",
    ]
    write(os.path.join(d, "f4-scripts.pdf"), pdf(objects, b"<< /Size %d /Root 1 0 R /Info 15 0 R >>"))


# ---------------------------------------------------------------- JPEG

def jpeg_insert_app1(jpeg, payload):
    """Insert an APP1 segment straight after SOI and any APP0."""
    at = 2
    if jpeg[2:4] == b"\xff\xe0":
        at = 4 + struct.unpack(">H", jpeg[4:6])[0]
    return jpeg[:at] + b"\xff\xe1" + struct.pack(">H", len(payload) + 2) + payload + jpeg[at:]


def long_orientation_exif(value, order):
    """An EXIF block whose only field is the orientation, typed LONG."""
    e = ">" if order == b"MM" else "<"
    tiff = order + struct.pack(e + "HI", 42, 8) + struct.pack(e + "H", 1)
    tiff += struct.pack(e + "HHII", 0x0112, 4, 1, value) + struct.pack(e + "I", 0)
    return b"Exif\0\0" + tiff


def mpf_index(sizes, attrs, primary_len, tiff_base):
    entries = b""
    at = primary_len
    for k, (size, attr) in enumerate(zip(sizes, attrs, strict=True)):
        offset = 0 if k == 0 else at - tiff_base
        if k:
            at += size
        entries += struct.pack(">IIIHH", attr, size, offset, 0, 0)
    head = b"MPF\0" + b"MM\x00\x2a\x00\x00\x00\x08" + struct.pack(">H", 3)
    head += struct.pack(">HHI4s", 0xB000, 7, 4, b"0100")
    head += struct.pack(">HHII", 0xB001, 4, 1, len(sizes))
    head += struct.pack(">HHII", 0xB002, 7, 16 * len(sizes), 50)
    head += struct.pack(">I", 0)
    return head + entries


def make_mpo(d):
    """A stereo pair: two views, each typed as a disparity image, the first
    flagged as the one to show."""
    left = read(os.path.join(d, "base.jpg"))
    right = read(os.path.join(d, "right.jpg"))
    attrs = [0x20020002, 0x00020002]
    placeholder = mpf_index([0, 0], attrs, 0, 0)
    at = 4 + struct.unpack(">H", left[4:6])[0] if left[2:4] == b"\xff\xe0" else 2
    seg = b"\xff\xe2" + struct.pack(">H", len(placeholder) + 2)
    primary_len = len(left) + len(seg) + len(placeholder)
    tiff_base = at + len(seg) + 4
    index = mpf_index([primary_len, len(right)], attrs, primary_len, tiff_base)
    write(os.path.join(d, "f8-stereo.jpg"), left[:at] + seg + index + left[at:] + right)


def make_jpegs(d):
    base = os.path.join(d, "base.png")
    sips(base, "jpeg", os.path.join(d, "base.jpg"))
    write(os.path.join(d, "right.png"), png_bytes(16, 12, gradient(16, 12, 90)))
    sips(os.path.join(d, "right.png"), "jpeg", os.path.join(d, "right.jpg"))
    jpeg = read(os.path.join(d, "base.jpg"))
    write(os.path.join(d, "f6-long.jpg"), jpeg_insert_app1(jpeg, long_orientation_exif(6, b"MM")))
    write(os.path.join(d, "f6-long-ii.jpg"), jpeg_insert_app1(jpeg, long_orientation_exif(6, b"II")))
    shutil.copy(os.path.join(d, "base.jpg"), os.path.join(d, "f9-comment.jpg"))
    exiftool_write(os.path.join(d, "f9-comment.jpg"), "-Comment=%PDF-1.7 scanned by " + AUTHOR.decode())
    sips(base, "jpeg", os.path.join(d, "f7-icc.jpg"), "--embedProfile",
         "/System/Library/ColorSync/Profiles/Display P3.icc")
    make_mpo(d)


def make_orientation(d, squint):
    shutil.copy(os.path.join(d, "base.png"), os.path.join(d, "f2-orient6.png"))
    exiftool_write(os.path.join(d, "f2-orient6.png"), "-n", "-Orientation=6")
    webp = os.path.join(d, "base.webp")
    r = run([squint, os.path.join(d, "base.png"), "--format", "webp", "--out", webp])
    if r.returncode != 0:
        sys.exit("squint could not write the base WebP")
    shutil.copy(webp, os.path.join(d, "f2-orient6.webp"))
    exiftool_write(os.path.join(d, "f2-orient6.webp"), "-n", "-Orientation=6")


def make(d, squint):
    os.makedirs(d, exist_ok=True)
    write(os.path.join(d, "base.png"), png_bytes(16, 12, gradient(16, 12, 0)))
    make_gifs(d)
    make_orientation(d, squint)
    make_heifs(d)
    make_pdf(d)
    make_jpegs(d)
    bmp_noise(os.path.join(d, "race.bmp"), 3000, 2000)
    sips(os.path.join(d, "race.bmp"), "jpeg", os.path.join(d, "f10-race.jpg"), "-s", "formatOptions", "95")
    os.remove(os.path.join(d, "race.bmp"))
    print(f"fixtures written to {d}")


# ---------------------------------------------------------------- judging

class Verdicts:
    def __init__(self):
        self.defects = 0

    def check(self, finding, defect, evidence):
        self.defects += bool(defect)
        print(f"{finding:<4} {'DEFECT' if defect else 'clean '}  {evidence}")


def squint_run(squint, src, out, *args):
    if os.path.exists(out):
        os.remove(out)
    r = run([squint, src, *args, "--out", out])
    return r.returncode, (r.stdout + r.stderr).decode(errors="replace").strip()


def probe(*paths):
    r = run(["swift", PROBE, *paths])
    found = {}
    for line in r.stdout.decode().splitlines():
        path, _, rest = line.partition(" ")
        found[path] = dict(kv.split("=", 1) for kv in rest.split())
    return found


def exif_tags(path, *tags):
    r = run([EXIFTOOL, "-a", "-G1", "-u", "-s", *tags, path])
    lines = r.stdout.decode(errors="replace").splitlines()
    return [ln for ln in lines if not re.match(r"\[(System|ExifTool|File|Composite)\]", ln)]


def refused(v, finding, name, text, carried):
    """Judge a refusal. "Nothing to remove" says the file is already clean,
    so it is the defect when the file still carries what was planted; any
    other refusal leaves the file alone without claiming anything."""
    last = text.splitlines()[-1]
    v.check(finding, carried and "nothing to remove" in last, f"{name}: refused ({last}); payload in source={carried}")


def judge_gif(v, d, squint):
    for name in ["f1-gce.gif", "f1-plaintext.gif"]:
        src, out = os.path.join(d, name), os.path.join(d, "out-" + name)
        code, text = squint_run(squint, src, out, "--mode", "strip")
        if code != 0:
            refused(v, "1", name, text, PERSON in read(src))
            continue
        kept = PERSON in read(out)
        drawn = probe(out).get(out, {}).get("rendered", "none")
        v.check("1", kept or drawn == "none", f"{name}: payload in output={kept}, Image I/O draws it at {drawn}")


def judge_orientation(v, d, squint):
    for name in ["f2-orient6.webp", "f2-orient6.png"]:
        src = os.path.join(d, name)
        ext = name.rsplit(".", 1)[1]
        stripped = os.path.join(d, "out-strip-" + name)
        squint_run(squint, src, stripped, "--mode", "strip")
        outs = [stripped]
        if ext == "png":
            for q in ["70", "-1"]:
                o = os.path.join(d, f"out-fast{q}-" + name)
                squint_run(squint, src, o, "--mode", "fast", "--png-quality", q)
                outs.append(o)
        seen = probe(src, *outs)
        want = seen[src]["rendered"]
        for o in outs:
            got = seen.get(o, {}).get("rendered", "missing")
            v.check("2", got != want, f"{os.path.basename(o)}: drawn {got}, source drawn {want}")


def judge_heif(v, d, squint):
    marks = {"f3-xml.heic": XMP_MARK, "f3-keys.heic": KEYS_MARK, "f3-name.heic": ITEM_NAME}
    for name, mark in marks.items():
        src, out = os.path.join(d, name), os.path.join(d, "out-" + name)
        code, text = squint_run(squint, src, out, "--mode", "strip")
        if code != 0:
            refused(v, "3", name, text, mark in read(src))
            continue
        before = [t for t in exif_tags(src) if mark.decode() in t]
        after = [t for t in exif_tags(out) if mark.decode() in t]
        in_bytes = mark in read(out)
        drawn = probe(out).get(out, {}).get("rendered", "none")
        v.check("3", in_bytes or bool(after) or drawn == "none",
                f"{name}: exiftool shows it before={len(before)} after={len(after)}, "
                f"bytes after={in_bytes}, Image I/O draws {drawn}")


def inflated(pdf_bytes):
    """The file with every stream that inflates appended, inflated."""
    text = bytearray(pdf_bytes)
    for m in re.finditer(rb"stream\r?\n", pdf_bytes):
        body = pdf_bytes[m.end():]
        try:
            text += zlib.decompressobj().decompress(body)
        except zlib.error:
            continue
    return bytes(text)


PDF_SCRIPT_KEYS = [rb"/JS\b", rb"/JavaScript\b", rb"/OpenAction\b", rb"/AA\b"]


def judge_pdf(v, d, squint):
    src, out = os.path.join(d, "f4-scripts.pdf"), os.path.join(d, "out-f4-scripts.pdf")
    code, text = squint_run(squint, src, out, "--mode", "strip")
    if code != 0:
        v.check("4", True, f"strip refused: {text}")
        return
    body = inflated(read(out))
    for key in PDF_SCRIPT_KEYS:
        n = len(re.findall(key, body))
        v.check("4", n > 0, f"{key.decode()} occurrences in output: {n}")
    for mark, what in [(AUTHOR, "author name"), (DATE, "annotation date")]:
        n = body.count(mark)
        v.check("4", n > 0, f"{what} occurrences in output: {n}")
    pages = probe(out).get(out, {}).get("pages", "unreadable")
    v.check("4", pages != "1", f"Quartz opens the output: pages={pages}")
    print(f"     result line: {text.splitlines()[0]}")


def judge_long_orientation(v, d, squint):
    for order in ["", "-ii"]:
        src = os.path.join(d, f"f6-long{order}.jpg")
        stripped = os.path.join(d, f"out-strip-f6{order}.jpg")
        fast = os.path.join(d, f"out-fast-f6{order}.jpg")
        squint_run(squint, src, stripped, "--mode", "strip")
        squint_run(squint, src, fast, "--mode", "fast")
        seen = probe(src, stripped, fast)
        want = seen[src]["rendered"]
        for o in [stripped, fast]:
            got = seen.get(o, {}).get("rendered", "missing")
            v.check("6", got != want, f"{os.path.basename(o)}: drawn {got}, source drawn {want}")
        tag = exif_tags(stripped, "-Orientation", "-n")
        bad = any(not re.search(r":\s*[1-8]\s*$", t) for t in tag)
        v.check("6", bad, f"exiftool Orientation in {os.path.basename(stripped)}: {tag or 'none'}")


def judge_icc(v, d, squint):
    src, out = os.path.join(d, "f7-icc.jpg"), os.path.join(d, "out-f7-icc.jpg")
    squint_run(squint, src, out, "--mode", "strip")
    desc = exif_tags(out, "-ICC_Profile:ProfileDescription", "-ICC_Profile:ProfileCopyright")
    print(f"7    kept    profile text after Strip, by design: {desc}")


def judge_mpo(v, d, squint):
    src, out = os.path.join(d, "f8-stereo.jpg"), os.path.join(d, "out-f8-stereo.jpg")
    code, text = squint_run(squint, src, out, "--mode", "strip")
    if code != 0:
        v.check("8", True, f"strip refused: {text}")
        return
    before = [t.split(":", 1)[1].strip() for t in exif_tags(src, "-MPImageType")]
    after = [t.split(":", 1)[1].strip() for t in exif_tags(out, "-MPImageType")]
    v.check("8", before != after, f"MPImageType before={before} after={after}")


def judge_pdf_marker(v, d, squint):
    src, out = os.path.join(d, "f9-comment.jpg"), os.path.join(d, "out-f9-comment.jpg")
    code, text = squint_run(squint, src, out, "--mode", "strip")
    comment = exif_tags(out, "-Comment") if code == 0 else []
    v.check("9", code != 0, f"strip exit {code}: {text.splitlines()[-1]}; Comment after: {comment or 'none'}")


def judge_race(v, d, squint):
    src = os.path.join(d, "f10-race.jpg")
    work = os.path.join(d, "out-race.jpg")
    shutil.copy(src, work)
    edit = read(os.path.join(d, "base.jpg"))
    proc = subprocess.Popen([squint, work, "--mode", "quality", "--out", work],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    time.sleep(1.5)
    still_running = proc.poll() is None
    write(work, edit)
    out, err = proc.communicate()
    kept = read(work) == edit
    if not still_running:
        v.check("10", False, "inconclusive: the run finished before the edit landed")
        return
    v.check("10", not kept, f"edit made mid-run survived={kept}; squint said: "
            f"{(out + err).decode(errors='replace').strip().splitlines()[-1]}")


def judge(d, squint):
    v = Verdicts()
    for step in [judge_gif, judge_orientation, judge_heif, judge_pdf, judge_long_orientation,
                 judge_icc, judge_mpo, judge_pdf_marker, judge_race]:
        step(v, d, squint)
    print(f"{v.defects} check(s) found the defect")
    return 1 if v.defects else 0


# ---------------------------------------------------------------- memory

def peak_rss(args):
    r = run(["/usr/bin/time", "-l", *args])
    m = re.search(rb"(\d+)\s+maximum resident set size", r.stderr)
    return int(m.group(1)) if m else None, r


def measure(d, squint):
    os.makedirs(d, exist_ok=True)
    for w, h in [(1500, 1000), (2121, 1414), (3000, 2000), (4243, 2828)]:
        bmp, jpg = os.path.join(d, "m.bmp"), os.path.join(d, f"m-{w}x{h}.jpg")
        if not os.path.exists(jpg):
            bmp_noise(bmp, w, h)
            sips(bmp, "jpeg", jpg, "-s", "formatOptions", "90")
            os.remove(bmp)
        for mode in ["fast", "quality"]:
            rss, r = peak_rss([squint, jpg, "--mode", mode, "--out", os.path.join(d, "m-out.jpg")])
            said = r.stdout.decode(errors="replace").strip().splitlines()
            print(f"{w}x{h} {w * h / 1e6:5.2f} MP {mode:<7} peak {rss / 1e9:6.3f} GB"
                  f"  {rss / (w * h):6.1f} B/px  {said[-2].strip() if len(said) > 1 else ''}")


def main():
    if len(sys.argv) != 4 or sys.argv[1] not in ("make", "judge", "measure"):
        sys.exit(__doc__)
    command, d, squint = sys.argv[1], os.path.abspath(sys.argv[2]), os.path.abspath(sys.argv[3])
    if command == "make":
        make(d, squint)
    elif command == "judge":
        sys.exit(judge(d, squint))
    else:
        measure(d, squint)


if __name__ == "__main__":
    main()
