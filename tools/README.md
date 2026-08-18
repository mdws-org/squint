# Tools

`jpeg-segments.py` lists the marker segments in a JPEG and reports which metadata
strings are present in the bytes.

```
python3 tools/jpeg-segments.py original.jpg processed.jpg
```

Use it to confirm metadata is actually gone. Finder's Get Info panel is cached and
will keep reporting camera and location data after a file is processed, so it
cannot be used to verify this. Reading the bytes can.

It found a real defect during development: a first implementation of Strip mode
copied everything from the start-of-scan marker to the end of the file, and XMP
survived, because Apple writes trailing data past the end-of-image marker.
