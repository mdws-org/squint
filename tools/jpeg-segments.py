import sys
NAMES = {0xE0:"APP0 JFIF",0xE1:"APP1 EXIF/XMP",0xE2:"APP2 ICC/MPF",0xEB:"APP11 C2PA/JUMBF",
         0xEA:"APP10 AROT",0xED:"APP13 IPTC",0xEE:"APP14 Adobe"}
NEEDLES = [b"GPS", b"Exif", b"ns.adobe.com/xap", b"jumb", b"c2pa", b"Photoshop", b"ICC_PROFILE"]
for path in sys.argv[1:]:
    d = open(path, "rb").read()
    print("--- %s (%d bytes) ---" % (path, len(d)))
    i = 2
    while i + 4 <= len(d) and d[i] == 0xFF:
        m = d[i+1]
        if m in (0xD8, 0x01) or 0xD0 <= m <= 0xD7:
            i += 2; continue
        if m in (0xDA, 0xD9): break
        ln = int.from_bytes(d[i+2:i+4], "big")
        tag = d[i+4:i+18].split(b"\x00")[0][:14]
        if 0xE0 <= m <= 0xEF:
            print("  FF%02X %-18s %6d B  %s" % (m, NAMES.get(m, "APPn"), ln, tag))
        i += 2 + ln
    found = [n.decode("ascii", "replace") for n in NEEDLES if n in d]
    print("  strings present: %s" % (", ".join(found) if found else "none"))
    print()
