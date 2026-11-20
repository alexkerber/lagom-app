import Foundation
import Observation

/// Today, Todo, Calendar and mailbox counts. Core owns the task and event rules; this holds what the pages show.
@MainActor @Observable
final class PlannerModel {
    var core: LagomCore?
    var today: TodaySummary?
    var lists: [TodoList] = []
    /// Each list's tasks, open first, as core orders them.
    var todos: [Int64: [Todo]] = [:]
    var agenda: [AgendaItem] = []
    /// iPad Calendar's week: its Monday and that week's events.
    // shortcut: a time zone change keeps the old Monday's instant until Today is tapped; re-anchor on NSSystemTimeZoneDidChange if travellers report it.
    var weekStart = PlannerModel.monday(Date())
    var week: [AgendaItem] = []
    var calendarConnected = false
    /// Google accounts that still need calendar access: Calendar and Today offer it.
    var calendarAccess: [MailAccount] = []
    /// A Google account allows calendar access; until its first fetch Calendar waits for the sync.
    var calendarGranted = false
    /// Writable Google calendars; New event shows only when there is one. The demo has the Mac's "Demo calendar".
    var eventCalendars: [EventCalendar] = []
    var counts: [MailCounts] = []
    var error: String?
    /// How far ahead Calendar lists cached events.
    static let agendaDays: UInt32 = 14

    var unreadCount: Int { Int(today?.unreadCount ?? 0) }
    /// Inbox mail left in one account, or across all of them.
    func inboxLeft(account: String?) -> UInt64 {
        counts.filter { account == nil || $0.account == account }.reduce(0) { $0 + $1.inbox }
    }
    func later(account: String?) -> UInt64 {
        counts.filter { account == nil || $0.account == account }.reduce(0) { $0 + $1.later }
    }
    var defaultList: Int64? { lists.first(where: \.isDefault)?.id ?? lists.first?.id }
    /// Open tasks in one list, the count beside its heading.
    func openCount(list: Int64) -> Int { todos[list]?.filter { !$0.done }.count ?? 0 }
    /// The stored message a task came from; nil, with the reason shown, when it is gone.
    func mail(for todo: Todo) async -> Int64? {
        guard let core else { return nil }
        do {
            if let id = try await core.todoMail(id: todo.id) { return id }
            error = "The linked mail is no longer available."
        } catch { self.error = error.localizedDescription }
        return nil
    }
    /// Lists are loaded and none has an open task: To-do shows its clear-day sentence.
    var allClear: Bool { !lists.isEmpty && lists.allSatisfy { todos[$0.id]?.contains { !$0.done } == false } }

    func reload() async {
        await reloadMail()
        await reloadTodos()
        await reloadAgenda()
    }

    /// Today's summary and the mailbox counts; cheap, so mail changes call it.
    func reloadMail() async {
        guard let core else { return }
        do {
            let today = try await core.today(unreadLimit: 3)
            let counts = try await core.mailCounts()
            self.today = today
            self.counts = counts
        } catch { self.error = error.localizedDescription }
    }

    func reloadTodos() async {
        guard let core else { return }
        do {
            let lists = try await core.todoLists()
            var todos: [Int64: [Todo]] = [:]
            for list in lists { todos[list.id] = try await core.todos(list: list.id, includeDone: true) }
            self.lists = lists
            self.todos = todos
            today = try await core.today(unreadLimit: 3)
        } catch { self.error = error.localizedDescription }
    }

    func reloadAgenda() async {
        guard let core else { return }
        do {
            let agenda = try await core.agenda(startDay: DateWords().coreDay(Date()), days: Self.agendaDays)
            calendarConnected = try await core.calendarConnected()
            calendarAccess = try await core.calendarAccessNeeded()
            calendarGranted = try await core.calendarGranted()
            // The demo's calendar only in the store that has one, not in `-LagomDemoCalendar`'s empty states.
            eventCalendars = Storage.demo ? (calendarConnected ? [EventCalendar(account: "", calendar: "demo", label: "Demo calendar")] : [])
                : try await core.eventCalendars()
            self.agenda = agenda
        } catch { self.error = error.localizedDescription }
        await reloadWeek()
    }

    private var weekLoad = 0

    func reloadWeek() async {
        guard let core else { return }
        weekLoad += 1
        let load = weekLoad
        do {
            let week = try await core.agenda(startDay: DateWords().coreDay(weekStart), days: 7)
            if load == weekLoad { self.week = week } // only the latest request lands
        } catch { if load == weekLoad { self.error = error.localizedDescription } }
    }

    /// Moves the week by `weeks`; 0 goes back to this week.
    func showWeek(_ weeks: Int) async {
        weekStart = weeks == 0 ? Self.monday(Date()) : Calendar.current.date(byAdding: .day, value: 7 * weeks, to: weekStart) ?? weekStart
        week = []
        await reloadWeek()
    }

    /// Saves a new event; nil on success, else the core's words for the form. Google's copy is cached,
    /// so the week shows it at once.
    func createEvent(_ form: EventForm, in target: EventCalendar) async -> String? {
        guard let core else { return "No calendar is open" }
        do {
            if Storage.demo { try await core.addDemoEvent(form: form) }
            else { try await core.createEvent(accountId: target.account, calendar: target.calendar, form: form) }
        } catch { return error.localizedDescription }
        await reloadAgenda()
        return nil
    }

    /// Desktop `new_event_default`: on today the next quarter hour (at most 23:45), on another day 09:00; an hour long.
    nonisolated static func newEventTimes(day: Date, now: Date = .now, calendar: Calendar = .current) -> (start: Date, end: Date) {
        let midnight = calendar.startOfDay(for: day)
        let minute: Int
        if calendar.isDate(day, inSameDayAs: now) {
            let c = calendar.dateComponents([.hour, .minute], from: now)
            minute = min(((c.hour ?? 0) * 60 + (c.minute ?? 0) + 14) / 15 * 15, 1425)
        } else { minute = 9 * 60 }
        let start = calendar.date(byAdding: .minute, value: minute, to: midnight) ?? midnight
        return (start, start.addingTimeInterval(3600))
    }

    /// The form core reads: instants for a timed event, the first and last day for an all-day one.
    nonisolated static func eventForm(title: String, allDay: Bool, start: Date, end: Date, guests: String, description: String,
                                      meet: Bool, calendar: Calendar = .current) -> EventForm {
        let words = DateWords(calendar: calendar)
        return EventForm(title: title, allDay: allDay, startUnix: Int64(start.timeIntervalSince1970.rounded(.down)),
                         endUnix: Int64(end.timeIntervalSince1970.rounded(.down)), firstDay: words.coreDay(start), lastDay: words.coreDay(end),
                         timeZone: calendar.timeZone.identifier, guests: guests, description: description, meet: meet)
    }

    nonisolated static func monday(_ date: Date, calendar: Calendar = .current) -> Date {
        let day = calendar.startOfDay(for: date)
        let sinceMonday = (calendar.component(.weekday, from: day) + 5) % 7
        return calendar.date(byAdding: .day, value: -sinceMonday, to: day) ?? day
    }

    /// Returns false, without asking core, for a blank title.
    @discardableResult
    func add(title: String, list: Int64, due: Date?) async -> Bool {
        let title = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let core, !title.isEmpty else { return false }
        do {
            _ = try await core.addTodo(list: list, title: title, dueDate: due.map { DateWords().coreDay($0) })
            await reloadTodos()
            return true
        } catch {
            self.error = error.localizedDescription
            return false
        }
    }

    /// Saves at once; `settle` keeps the lists as drawn a moment longer, so a tick is seen before its row moves.
    func toggle(_ todo: Todo, settle: Duration = .zero) async {
        guard let core else { return }
        do { try await core.setTodoDone(id: todo.id, done: !todo.done) } catch { self.error = error.localizedDescription }
        if settle > .zero { try? await Task.sleep(for: settle) }
        await reloadTodos()
    }

    func delete(_ todo: Todo) async {
        guard let core else { return }
        do { try await core.deleteTodo(id: todo.id) } catch { self.error = error.localizedDescription }
        await reloadTodos()
    }

    func addList(name: String) async {
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let core, !name.isEmpty else { return }
        do { _ = try await core.addTodoList(name: name) } catch { self.error = error.localizedDescription }
        await reloadTodos()
    }
}
