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

    /// DEBUG builds launched with `-LagomIntroChoose` open the intro on Choose account, for screenshots without taps.
    static var introChoose: Bool {
        #if DEBUG
        ProcessInfo.processInfo.arguments.contains("-LagomIntroChoose")
        #else
        false
        #endif
    }

    /// DEBUG builds launched with `-LagomDemoSettings list|account` open Settings, or its first account's page, for screenshots without taps.
    static var settingsScreen: String? {
        #if DEBUG
        UserDefaults.standard.string(forKey: "LagomDemoSettings")
        #else
        nil
        #endif
    }

    /// DEBUG builds launched with `-LagomDemoNewEvent empty|filled|saved` open Calendar's New event sheet, filled in,
    /// or filled in and saved, for screenshots without taps.
    static var newEventScreen: String? {
        #if DEBUG
        demo ? UserDefaults.standard.string(forKey: "LagomDemoNewEvent") : nil
        #else
        nil
        #endif
    }

    static func directory() throws -> URL {
        if demo {
            // `-LagomDemoCalendar google|granted|none`: Calendar's empty states, for screenshots.
            let calendar = UserDefaults.standard.string(forKey: "LagomDemoCalendar")
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent("LagomDemo" + (calendar ?? ""), isDirectory: true)
            if let calendar {
                try? FileManager.default.removeItem(at: directory)
                try seedDemoWithoutCalendar(dataDir: directory.path, state: calendar)
            } else { try seedDemo(dataDir: directory.path) }
            return directory
        }
        return try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            .appendingPathComponent("Lagom", isDirectory: true)
    }
}
