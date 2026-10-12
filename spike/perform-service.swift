// SANDBOX SPIKE: invoke a Services entry on files, as Finder's right-click does.
//
//   perform-service "<menu title>" <file>...
import AppKit

let arguments = CommandLine.arguments
guard arguments.count >= 3 else {
    FileHandle.standardError.write("usage: perform-service <title> <file>...\n".data(using: .utf8)!)
    exit(2)
}
let urls = arguments.dropFirst(2).map { URL(fileURLWithPath: $0) as NSURL }
let pasteboard = NSPasteboard(name: NSPasteboard.Name("spike-\(UUID().uuidString)"))
pasteboard.clearContents()
pasteboard.writeObjects(urls)
let performed = NSPerformService(arguments[1], pasteboard)
print(performed ? "performed \(arguments[1]) on \(urls.count) file(s)" : "NSPerformService returned false")
exit(performed ? 0 : 1)
