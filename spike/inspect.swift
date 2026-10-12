// SANDBOX SPIKE: what a file holds, read through Image I/O and the filesystem.
//
//   inspect <file>...            print one line per file
//   inspect --tag <name> <file>  set a Finder tag
import Foundation
import ImageIO

let arguments = Array(CommandLine.arguments.dropFirst())
if arguments.first == "--tag", arguments.count == 3 {
    var url = URL(fileURLWithPath: arguments[2])
    var values = URLResourceValues()
    values.tagNames = [arguments[1]]
    try url.setResourceValues(values)
    exit(0)
}

for path in arguments {
    let url = URL(fileURLWithPath: path)
    let keys: Set<URLResourceKey> = [.fileSizeKey, .creationDateKey, .tagNamesKey]
    let values = try? url.resourceValues(forKeys: keys)
    var fields = [
        "bytes=\(values?.fileSize ?? -1)",
        "created=\(values?.creationDate.map { ISO8601DateFormatter().string(from: $0) } ?? "?")",
        "tags=\((values?.tagNames ?? []).joined(separator: ","))",
    ]
    if let source = CGImageSourceCreateWithURL(url as CFURL, nil),
       let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any] {
        fields.append("gps=\(properties[kCGImagePropertyGPSDictionary] != nil ? "YES" : "no")")
        fields.append("profile=\(properties[kCGImagePropertyProfileName] as? String ?? "none")")
        let hdr = CGImageSourceCopyAuxiliaryDataInfoAtIndex(source, 0, kCGImageAuxiliaryDataTypeISOGainMap) != nil
            || CGImageSourceCopyAuxiliaryDataInfoAtIndex(source, 0, kCGImageAuxiliaryDataTypeHDRGainMap) != nil
        fields.append("gainmap=\(hdr ? "yes" : "no")")
    } else {
        fields.append("UNREADABLE")
    }
    print("\(url.lastPathComponent) " + fields.joined(separator: " "))
}
