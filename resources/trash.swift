import Foundation

guard CommandLine.arguments.count == 2 else {
    fputs("usage: trash-helper <absolute-path>\n", stderr)
    exit(2)
}
let path = CommandLine.arguments[1]
guard path.hasPrefix("/") else {
    fputs("path must be absolute\n", stderr)
    exit(2)
}
do {
    var result: NSURL?
    try FileManager.default.trashItem(at: URL(fileURLWithPath: path), resultingItemURL: &result)
} catch {
    fputs("\(error)\n", stderr)
    exit(1)
}
