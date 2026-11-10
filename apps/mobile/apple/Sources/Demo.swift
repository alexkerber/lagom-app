import Foundation

/// Where the store lives. DEBUG builds launched with `-LagomDemo` open a seeded fixture store
/// instead (the desktop's `LAGOM_DEMO=1`), never the real one, and skip network sync.
enum Storage {
    static var demo: Bool {
        #if DEBUG
        ProcessInfo.processInfo.arguments.contains("-LagomDemo")
        #else
        false
        #endif
    }

    static func directory() throws -> URL {
        if demo {
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent("LagomDemo", isDirectory: true)
            try seedDemo(dataDir: directory.path)
            return directory
        }
        return try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            .appendingPathComponent("Lagom", isDirectory: true)
    }
}
