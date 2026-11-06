"""Writes the seed corpus for the `pdf` fuzz target into seeds/pdf/.

Without seeds the fuzzer never produces the `%PDF-` marker within its run
budget, so every input is turned away by `pdf::is_pdf` and lopdf is never
reached. Each seed is a small, well-formed document that exercises one part
of what Strip touches: the info dictionary and XMP packet, annotations and
PieceInfo, the file ID, images in each colour space and filter the module
reads, a JPEG carrying Exif, signatures and usage rights, attachments, a
cross-reference stream with an object stream, and an incremental update.

Standard library only; the output is the same on every run. Run from any
directory with `python3 make_pdf_seeds.py`.
"""

import pathlib
import struct
import zlib

OUT = pathlib.Path(__file__).resolve().parent / "pdf"


def stream(dict_body: bytes, data: bytes) -> bytes:
    return b"<< " + dict_body + b" /Length %d >>\nstream\n" % len(data) + data + b"\nendstream"


def classic(version: bytes, objects: dict, trailer: bytes) -> bytes:
    """A document with a classic xref table, offsets computed."""
    out = bytearray(b"%PDF-" + version + b"\n%\xe2\xe3\xcf\xd3\n")
    offsets = {}
    for num in sorted(objects):
        offsets[num] = len(out)
        out += b"%d 0 obj\n" % num + objects[num] + b"\nendobj\n"
    size = max(objects) + 1
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % size
    for num in range(1, size):
        if num in offsets:
            out += b"%010d 00000 n \n" % offsets[num]
        else:
            out += b"0000000000 65535 f \n"
    out += b"trailer\n<< /Size %d " % size + trailer + b" >>\nstartxref\n%d\n%%%%EOF\n" % xref
    return bytes(out)


def append_update(base: bytes, objects: dict, trailer: bytes) -> bytes:
    """An incremental update: new objects, a partial xref, /Prev to the old one."""
    prev = int(base.rsplit(b"startxref\n", 1)[1].split(b"\n", 1)[0])
    out = bytearray(base)
    offsets = {}
    for num in sorted(objects):
        offsets[num] = len(out)
        out += b"%d 0 obj\n" % num + objects[num] + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 1\n0000000000 65535 f \n"
    for num in sorted(objects):
        out += b"%d 1\n%010d 00000 n \n" % (num, offsets[num])
    size = max(max(objects) + 1, int(base.split(b"/Size ")[-1].split(b" ")[0]))
    out += b"trailer\n<< /Size %d /Prev %d " % (size, prev) + trailer + b" >>\nstartxref\n%d\n%%%%EOF\n" % xref
    return bytes(out)


def rgb_pixels(w: int, h: int) -> bytes:
    return bytes((x * 60 + y * 20) % 256 for y in range(h) for x in range(w) for _ in range(3))


def grey_png_predicted(w: int, h: int) -> bytes:
    """Rows each led by a PNG filter byte (0 none, 1 sub, 2 up, cycling)."""
    rows = bytearray()
    for y in range(h):
        rows.append(y % 3)
        rows += bytes((x * 30 + y * 10) % 256 if y % 3 == 0 else 7 for x in range(w))
    return bytes(rows)


def tiny_jpeg() -> bytes:
    """An 8x8 mid-grey baseline JPEG carrying an Exif APP1 and an XMP APP1.

    One 8x8 block whose level-shifted samples are all zero, so its DC
    difference and every AC coefficient are zero: the scan is the DC code for
    category 0 followed by the AC end-of-block code, one bit each.
    """
    def seg(marker: int, body: bytes) -> bytes:
        return struct.pack(">BBH", 0xFF, marker, len(body) + 2) + body

    # Exif: little-endian TIFF, IFD0 with Make, then a GPS IFD with latitude ref.
    make = b"SeedCam\x00"
    ifd0_at, gps_at = 8, 8 + 2 + 2 * 12 + 4
    make_at = gps_at + 2 + 12 + 4
    tiff = b"II*\x00" + struct.pack("<I", ifd0_at)
    tiff += struct.pack("<H", 2)
    tiff += struct.pack("<HHII", 0x010F, 2, len(make), make_at)
    tiff += struct.pack("<HHII", 0x8825, 4, 1, gps_at)
    tiff += struct.pack("<I", 0)
    tiff += struct.pack("<H", 1) + struct.pack("<HHI", 0x0001, 2, 2) + b"N\x00\x00\x00" + struct.pack("<I", 0)
    tiff += make
    xmp = b'http://ns.adobe.com/xap/1.0/\x00<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/></x:xmpmeta>'

    dqt = b"\x00" + bytes([1] * 64)
    sof = struct.pack(">BHHB", 8, 8, 8, 1) + b"\x01\x11\x00"
    dht_dc = b"\x00" + bytes([1] + [0] * 15) + b"\x00"
    dht_ac = b"\x10" + bytes([1] + [0] * 15) + b"\x00"
    sos = b"\x01\x01\x00\x00\x3f\x00"
    return (
        b"\xff\xd8"
        + seg(0xE1, b"Exif\x00\x00" + tiff)
        + seg(0xE1, xmp)
        + seg(0xDB, dqt)
        + seg(0xC0, sof)
        + seg(0xC4, dht_dc)
        + seg(0xC4, dht_ac)
        + seg(0xDA, sos)
        + b"\x3f"
        + b"\xff\xd9"
    )


XMP_PACKET = (
    b'<?xpacket begin="\xef\xbb\xbf" id="W5M0MpCehiHzreSzNTczkc9d"?>'
    b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
    b'<rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:CreatorTool="Seed Writer 1.0"/>'
    b'</rdf:RDF></x:xmpmeta><?xpacket end="w"?>'
)

INFO = b"<< /Title (Seed) /Author (A. Writer) /Creator (Seed Writer) /Producer (Seed Writer 1.0) /CreationDate (D:20260101120000Z) /ModDate (D:20260102120000Z) >>"
ID = b"/ID [<0123456789abcdef0123456789abcdef> <0123456789abcdef0123456789abcdef>]"


def metadata_and_annotations() -> bytes:
    pixels = zlib.compress(rgb_pixels(4, 4), 9)
    content = b"q 72 0 0 72 10 10 cm /Im1 Do Q BT /F1 12 Tf 10 90 Td (Seed) Tj ET"
    return classic(b"1.4", {
        1: b"<< /Type /Catalog /Pages 2 0 R /Metadata 6 0 R /PieceInfo << /Seed << /LastModified (D:20260101) /Private (x) >> >> >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /Im1 5 0 R >> /Font << /F1 9 0 R >> >> /Contents 4 0 R /Annots [8 0 R 10 0 R] /PieceInfo << /Seed << /LastModified (D:20260101) /Private (y) >> >> /LastModified (D:20260101120000Z) >>",
        4: stream(b"", content),
        5: stream(b"/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode", pixels),
        6: stream(b"/Type /Metadata /Subtype /XML", XMP_PACKET),
        7: INFO,
        8: b"<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (move this up) /T (A. Writer) /M (D:20260101120000Z) /CreationDate (D:20260101120000Z) /Popup 11 0 R >>",
        9: b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        10: b"<< /Type /Annot /Subtype /Link /Rect [0 0 20 20] /Border [0 0 0] /A << /S /URI /URI (https://example.org/) >> >>",
        11: b"<< /Type /Annot /Subtype /Popup /Rect [30 30 90 90] /Parent 8 0 R /M (D:20260101120000Z) >>",
    }, b"/Root 1 0 R /Info 7 0 R " + ID)


def images() -> bytes:
    icc = zlib.compress(b"\x00" * 128, 9)
    rgb = zlib.compress(rgb_pixels(4, 4), 9)
    grey = zlib.compress(grey_png_predicted(4, 4), 9)
    alpha = zlib.compress(bytes(range(0, 256, 16)), 9)
    content = b"q 40 0 0 40 0 0 cm /Im1 Do Q q 40 0 0 40 50 0 cm /Im2 Do Q q 40 0 0 40 0 50 cm /Im3 Do Q q 40 0 0 40 50 50 cm /Im4 Do Q"
    rle = bytes([15]) + bytes(range(16)) + b"\x80"
    return classic(b"1.6", {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>",
        3: b"<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im1 5 0 R /Im2 6 0 R /Im3 8 0 R /Im4 9 0 R >> >> /Contents 4 0 R >>",
        4: stream(b"", content),
        5: stream(b"/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace [/ICCBased 7 0 R] /BitsPerComponent 8 /Filter /FlateDecode /SMask 10 0 R", rgb),
        6: stream(b"/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /DecodeParms << /Predictor 15 /Colors 1 /BitsPerComponent 8 /Columns 4 >>", grey),
        7: stream(b"/N 3 /Alternate /DeviceRGB /Filter /FlateDecode", icc),
        8: stream(b"/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /RunLengthDecode", rle),
        9: stream(b"/Type /XObject /Subtype /Image /Width 8 /Height 1 /ImageMask true /BitsPerComponent 1", b"\xaa"),
        10: stream(b"/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode", alpha),
    }, b"/Root 1 0 R " + ID)


def jpeg_with_exif() -> bytes:
    jpeg = tiny_jpeg()
    return classic(b"1.4", {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Im1 5 0 R /Im2 6 0 R >> >> /Contents 4 0 R >>",
        4: stream(b"", b"q 300 0 0 300 50 50 cm /Im1 Do Q q 100 0 0 100 400 400 cm /Im2 Do Q"),
        5: stream(b"/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode", jpeg),
        6: stream(b"/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter [/FlateDecode /DCTDecode] /DecodeParms [null << /ColorTransform 0 >>]", zlib.compress(jpeg, 9)),
    }, b"/Root 1 0 R /Info << /Producer (Seed Writer) >>")


def forms_signatures_attachments() -> bytes:
    attached = b"attached text\n"
    sig_contents = b"<" + b"00" * 32 + b">"
    return classic(b"1.7", {
        1: b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 8 0 R] /SigFlags 3 >> /Names << /EmbeddedFiles << /Names [(note.txt) 10 0 R] >> >> /Perms << /UR3 12 0 R >> /AF [10 0 R] >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [6 0 R 8 0 R 9 0 R] >>",
        4: stream(b"", b""),
        6: b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (A. Writer) /Rect [10 10 100 30] /P 3 0 R /M (D:20260101120000Z) >>",
        7: b"<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /Name (A. Writer) /M (D:20260101120000Z) /Reason (seed) /ByteRange [0 10 20 10] /Contents " + sig_contents + b" >>",
        8: b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (sig) /V 7 0 R /Rect [10 40 100 60] /P 3 0 R >>",
        9: b"<< /Type /Annot /Subtype /FileAttachment /Rect [150 150 170 170] /FS 10 0 R /T (A. Writer) /Contents (attached) >>",
        10: b"<< /Type /Filespec /F (note.txt) /UF (note.txt) /EF << /F 11 0 R /UF 11 0 R >> /AFRelationship /Data >>",
        11: stream(b"/Type /EmbeddedFile /Subtype /text#2Fplain /Params << /Size %d /ModDate (D:20260101120000Z) >>" % len(attached), attached),
        12: b"<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.s5 /Reference [<< /Type /SigRef /TransformMethod /UR3 /TransformParams << /Type /TransformParams /V /2.2 /Document [/FullSave] >> >>] /ByteRange [0 10 20 10] /Contents " + sig_contents + b" >>",
        13: b"<< /Type /DocTimeStamp /SubFilter /ETSI.RFC3161 /ByteRange [0 10 20 10] /Contents " + sig_contents + b" >>",
    }, b"/Root 1 0 R /Info << /Author (A. Writer) >> " + ID)


def xref_stream_with_object_stream() -> bytes:
    """PDF 1.5: dictionaries packed in an object stream, a compressed xref."""
    packed = {
        3: b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Annots [7 0 R] >>",
        4: b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R /MediaBox [0 0 50 80] /Rotate 90 >>",
        5: b"<< /Title (Packed) /Author (A. Writer) /ModDate (D:20260101120000Z) >>",
        7: b"<< /Type /Annot /Subtype /Square /Rect [5 5 20 20] /T (A. Writer) /M (D:20260101120000Z) >>",
    }
    header, body = bytearray(), bytearray()
    for num, obj in packed.items():
        header += b"%d %d " % (num, len(body))
        body += obj + b"\n"
    objstm_data = bytes(header) + bytes(body)

    out = bytearray(b"%PDF-1.5\n%\xe2\xe3\xcf\xd3\n")
    offsets = {}

    def put(num, obj):
        offsets[num] = len(out)
        out.extend(b"%d 0 obj\n" % num + obj + b"\nendobj\n")

    put(1, b"<< /Type /Catalog /Pages 2 0 R /Metadata 9 0 R >>")
    put(2, b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 100 100] >>")
    put(6, stream(b"", b"0 0 m 50 50 l S"))
    put(8, stream(b"/Type /ObjStm /N %d /First %d /Filter /FlateDecode" % (len(packed), len(header)), zlib.compress(objstm_data, 9)))
    put(9, stream(b"/Type /Metadata /Subtype /XML", XMP_PACKET))
    xref_num = 10
    offsets[xref_num] = len(out)

    rows = bytearray(b"\x00" + struct.pack(">IH", 0, 65535))
    index_in_stm = {num: i for i, num in enumerate(packed)}
    for num in range(1, xref_num + 1):
        if num in index_in_stm:
            rows += b"\x02" + struct.pack(">IH", 8, index_in_stm[num])
        else:
            rows += b"\x01" + struct.pack(">IH", offsets[num], 0)
    # PNG Up predictor over 7-byte rows, as most writers emit it.
    width, predicted, prev = 7, bytearray(), bytes(7)
    for i in range(0, len(rows), width):
        row = rows[i:i + width]
        predicted += b"\x02" + bytes((row[j] - prev[j]) % 256 for j in range(width))
        prev = row
    xref_dict = (
        b"/Type /XRef /Size %d /W [1 4 2] /Root 1 0 R /Info 5 0 R " % (xref_num + 1)
        + ID
        + b" /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 7 >>"
    )
    out += b"%d 0 obj\n" % xref_num + stream(xref_dict, zlib.compress(bytes(predicted), 9)) + b"\nendobj\n"
    out += b"startxref\n%d\n%%%%EOF\n" % offsets[xref_num]
    return bytes(out)


def incremental_update() -> bytes:
    """A signed first revision, then an update that drops the signature field."""
    sig_contents = b"<" + b"00" * 32 + b">"
    base = classic(b"1.7", {
        1: b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Annots [5 0 R] >>",
        4: stream(b"", b"0 0 m 100 100 l S"),
        5: b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (sig) /V 6 0 R /Rect [0 0 10 10] /P 3 0 R >>",
        6: b"<< /Type /Sig /Name (A. Writer) /M (D:20260101120000Z) /ByteRange [0 10 20 10] /Contents " + sig_contents + b" >>",
        7: INFO,
    }, b"/Root 1 0 R /Info 7 0 R " + ID)
    return append_update(base, {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>",
        8: b"<< /Title (Revised) /ModDate (D:20260103120000Z) >>",
    }, b"/Root 1 0 R /Info 8 0 R " + ID)


SEEDS = {
    "metadata-annotations.pdf": metadata_and_annotations,
    "images.pdf": images,
    "jpeg-exif.pdf": jpeg_with_exif,
    "forms-signatures-attachments.pdf": forms_signatures_attachments,
    "xref-stream.pdf": xref_stream_with_object_stream,
    "incremental-update.pdf": incremental_update,
}


def main() -> None:
    OUT.mkdir(exist_ok=True)
    for name, build in SEEDS.items():
        data = build()
        (OUT / name).write_bytes(data)
        print(f"{name}: {len(data)} bytes")


if __name__ == "__main__":
    main()
