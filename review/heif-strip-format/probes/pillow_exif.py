# /// script
# dependencies = ["pillow", "pillow-heif"]
# ///
"""Open each HEIC with Pillow + pillow-heif and call getexif(), as the Linux run did."""
import sys
from PIL import Image
import pillow_heif
pillow_heif.register_heif_opener()
for p in sys.argv[1:]:
    try:
        im = Image.open(p); im.load(); ex = im.getexif()
        print(f"{p}: getexif ok, {len(ex)} tags, size {im.size}")
    except Exception as e:
        print(f"{p}: {type(e).__name__}: {e}"[:200])
