// What Image I/O makes of each file named on the command line: the
// orientation it reads, and the size it draws the picture at once that
// orientation is applied. A PDF reports its page count instead.
//
//   swift tools/imageio-probe.swift a.webp b.png c.pdf
//
// Used by audit-2026-11-06.py as the outside judge of orientation, because
// a picture that keeps its pixels and loses its turn is only visibly wrong
// once something renders it.

import CoreGraphics
import Foundation
import ImageIO

for path in CommandLine.arguments.dropFirst() {
    let url = URL(fileURLWithPath: path)
    if url.pathExtension.lowercased() == "pdf" {
        let pages = CGPDFDocument(url as CFURL)?.numberOfPages
        print("\(path) pages=\(pages.map(String.init) ?? "unreadable")")
        continue
    }
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else {
        print("\(path) unreadable")
        continue
    }
    let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any] ?? [:]
    let orientation = (properties[kCGImagePropertyOrientation] as? Int).map(String.init) ?? "none"
    let options: [CFString: Any] = [
        kCGImageSourceCreateThumbnailFromImageAlways: true,
        kCGImageSourceCreateThumbnailWithTransform: true,
        kCGImageSourceThumbnailMaxPixelSize: 100_000,
    ]
    var rendered = "none"
    if let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) {
        rendered = "\(image.width)x\(image.height)"
    }
    print("\(path) orientation=\(orientation) rendered=\(rendered)")
}
