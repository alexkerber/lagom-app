import Foundation

/// The four top-level pages, as on desktop (⌘1–4). The raw value is what `@AppStorage` restores.
enum AppSection: String, CaseIterable, Identifiable {
    case today, mail, calendar, todo
    static let storageKey = "lagom.section"
    var id: String { rawValue }
    var title: String {
        switch self {
        case .today: "Today"
        case .mail: "Mail"
        case .calendar: "Calendar"
        case .todo: "Todo"
        }
    }
    /// The desktop sidebar icon.
    var icon: String {
        switch self {
        case .today: "home"
        case .mail: "mail"
        case .calendar: "calendar"
        case .todo: "todo"
        }
    }
}

/// Mail list and agenda wording, after the desktop's `mail::when`, `mail::date_group` and task due chips.
struct DateWords {
    var now = Date()
    var calendar = Calendar.current
    var locale = Locale.current

    private func days(from date: Date) -> Int {
        calendar.dateComponents([.day], from: calendar.startOfDay(for: date), to: calendar.startOfDay(for: now)).day ?? 0
    }
    private func format(_ date: Date, _ template: String) -> String {
        let formatter = DateFormatter()
        formatter.calendar = calendar
        formatter.timeZone = calendar.timeZone
        formatter.locale = locale
        formatter.setLocalizedDateFormatFromTemplate(template)
        return formatter.string(from: date)
    }

    /// The list's date column: the time today, "Yesterday", a weekday this week, "3 Nov" this year, else "Nov 2024".
    func listTime(_ date: Date) -> String {
        switch days(from: date) {
        case ...0: format(date, "jjmm")
        case 1: "Yesterday"
        case 2...6: format(date, "EEE")
        default: calendar.component(.year, from: date) == calendar.component(.year, from: now) ? format(date, "dMMM") : format(date, "MMMyyyy")
        }
    }

    /// The reading pane's date, `mail::reading_date`: "Today 09:42", "Fri 09:42", "3 Nov, 09:42".
    /// A date in the future, from a skewed sender clock, shows as a date.
    func readingDate(_ date: Date) -> String {
        let time = format(date, "jjmm")
        switch days(from: date) {
        case 0: return "Today \(time)"
        case 1...6: return "\(listTime(date)) \(time)"
        default:
            let thisYear = calendar.component(.year, from: date) == calendar.component(.year, from: now)
            return "\(format(date, thisYear ? "dMMM" : "dMMMyyyy")), \(time)"
        }
    }

    /// The list's group headers: Today, Yesterday, earlier weekdays of this week, then Earlier.
    func group(_ date: Date) -> String {
        let days = days(from: date)
        let weekday = (calendar.component(.weekday, from: now) + 5) % 7 // days since Monday
        switch days {
        case ...0: return "Today"
        case 1: return "Yesterday"
        case _ where days <= weekday: return format(date, "EEEE")
        default: return "Earlier"
        }
    }

    /// A due chip: relative words near today, then a date; `overdue` before today.
    func due(_ value: String) -> (label: String, overdue: Bool)? {
        guard let date = day(value) else { return nil }
        let offset = -days(from: date)
        let label = switch offset {
        case -1: "Yesterday"
        case 0: "Today"
        case 1: "Tomorrow"
        case 2...6: format(date, "EEEE")
        default: calendar.component(.year, from: date) == calendar.component(.year, from: now) ? format(date, "dMMM") : format(date, "dMMMyyyy")
        }
        return (label, offset < 0)
    }

    /// An agenda section title.
    func dayTitle(_ value: String) -> String {
        guard let date = day(value) else { return value }
        switch -days(from: date) {
        case 0: return "Today"
        case 1: return "Tomorrow"
        default: return format(date, "EEEEdMMMM")
        }
    }

    /// Core days are Gregorian whatever calendar the user reads them in.
    private var gregorian: Calendar {
        var gregorian = Calendar(identifier: .gregorian)
        gregorian.timeZone = calendar.timeZone
        return gregorian
    }

    /// A core day (`YYYY-MM-DD`, or the date part of an RFC 3339 instant) as local midnight.
    func day(_ value: String) -> Date? {
        let parts = value.prefix(10).split(separator: "-").compactMap { Int($0) }
        guard parts.count == 3 else { return nil }
        return gregorian.date(from: DateComponents(year: parts[0], month: parts[1], day: parts[2]))
    }

    /// The core form of a picked date.
    func coreDay(_ date: Date) -> String {
        let c = gregorian.dateComponents([.year, .month, .day], from: date)
        return String(format: "%04d-%02d-%02d", c.year ?? 0, c.month ?? 0, c.day ?? 0)
    }
}

struct RowGroup<Item>: Identifiable {
    let title: String
    var items: [Item]
    var id: String { title }
}

/// Consecutive rows that share a header, in the order given.
func grouped<Item>(_ items: [Item], by key: (Item) -> String) -> [RowGroup<Item>] {
    var groups: [RowGroup<Item>] = []
    for item in items {
        let title = key(item)
        if groups.last?.title == title { groups[groups.count - 1].items.append(item) } else { groups.append(RowGroup(title: title, items: [item])) }
    }
    return groups
}

/// A timed event placed on the week grid, desktop `week::layout`: minutes from midnight, and its lane among overlaps.
struct WeekBlock: Equatable {
    let index: Int
    let start: Double
    let end: Double
    var lane = 0
    var lanes = 1
}

/// Timed events on `day` as blocks at least 36 minutes tall; overlapping events share the width in lanes.
func weekBlocks(_ events: [AgendaItem], day: Date, calendar: Calendar = .current) -> [WeekBlock] {
    let midnight = calendar.startOfDay(for: day)
    guard let next = calendar.date(byAdding: .day, value: 1, to: midnight) else { return [] }
    let (from, to) = (Int64(midnight.timeIntervalSince1970), Int64(next.timeIntervalSince1970))
    func minute(_ unix: Int64) -> Double {
        let c = calendar.dateComponents([.hour, .minute, .second], from: Date(timeIntervalSince1970: TimeInterval(unix)))
        return Double(c.hour ?? 0) * 60 + Double(c.minute ?? 0) + Double(c.second ?? 0) / 60
    }
    // shortcut: a repeated DST hour shares one wall-clock row, as on desktop.
    var blocks = events.enumerated().filter { !$0.element.allDay && $0.element.startUnix < to && $0.element.endUnix > from }.map { i, e in
        let top = e.startUnix <= from ? 0 : minute(e.startUnix)
        let bottom = e.endUnix >= to ? 1440 : minute(e.endUnix)
        // A late event keeps its 36 minutes by starting earlier, so it never draws flat at the bottom.
        let start = min(top, 1440 - 36)
        return WeekBlock(index: i, start: start, end: min(max(bottom, start + 36), 1440))
    }
    blocks.sort { ($0.start, $0.end, $0.index) < ($1.start, $1.end, $1.index) }
    var group = 0, until = 0.0, lanes: [Double] = []
    for i in blocks.indices {
        if blocks[i].start >= until {
            for j in group..<i { blocks[j].lanes = lanes.count }
            group = i
            lanes.removeAll()
        }
        let lane = lanes.firstIndex { $0 <= blocks[i].start } ?? lanes.count
        if lane == lanes.count { lanes.append(blocks[i].end) } else { lanes[lane] = blocks[i].end }
        blocks[i].lane = lane
        until = group == i ? blocks[i].end : max(until, blocks[i].end)
    }
    for j in group..<blocks.count { blocks[j].lanes = lanes.count }
    return blocks
}
