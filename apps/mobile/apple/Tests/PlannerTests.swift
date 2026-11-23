import XCTest

final class PresentationTests: XCTestCase {
    private func words(now: String) -> DateWords {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        let base = DateWords(now: Date(), calendar: calendar, locale: Locale(identifier: "en_GB"))
        return DateWords(now: base.day(now)!.addingTimeInterval(10 * 3600), calendar: calendar, locale: base.locale)
    }
    private func at(_ words: DateWords, _ day: String, _ hour: Double = 9.7) -> Date { words.day(day)!.addingTimeInterval(hour * 3600) }

    func testAccountRowsSayNameThenKindAsOnTheMac() {
        XCTAssertEqual(accountKind(MailAccount(id: "a", email: "me@home.test", displayName: "Personal", kind: "Email and password")),
                       "Personal · Email and password")
        XCTAssertEqual(accountKind(MailAccount(id: "a", email: "me@home.test", displayName: "me@home.test", kind: "Google account")), "Google account")
        XCTAssertEqual(accountKind(MailAccount(id: "a", email: "me@home.test", displayName: "Personal")), "Personal")
    }

    func testListTimeFollowsTheDesktopDateColumn() {
        let w = words(now: "2026-11-10")
        XCTAssertEqual(w.listTime(at(w, "2026-11-10", 9.7)), "09:42")
        XCTAssertEqual(w.listTime(at(w, "2026-11-09")), "Yesterday")
        XCTAssertEqual(w.listTime(at(w, "2026-11-06")), "Fri")
        XCTAssertEqual(w.listTime(at(w, "2026-10-26")), "26 Oct")
        XCTAssertEqual(w.listTime(at(w, "2025-10-01")), "Oct 2025")
    }

    func testReadingDateFollowsTheDesktopReadingPane() {
        let w = words(now: "2026-11-10")
        XCTAssertEqual(w.readingDate(at(w, "2026-11-10")), "Today 09:42")
        XCTAssertEqual(w.readingDate(at(w, "2026-11-09")), "Yesterday 09:42")
        XCTAssertEqual(w.readingDate(at(w, "2026-11-06")), "Fri 09:42")
        XCTAssertEqual(w.readingDate(at(w, "2026-10-26")), "26 Oct, 09:42")
        XCTAssertEqual(w.readingDate(at(w, "2025-10-01")), "1 Oct 2025, 09:42")
        // A sender clock ahead of ours: a date, never "Today".
        XCTAssertEqual(w.readingDate(at(w, "2026-11-12")), "12 Nov, 09:42")
    }

    /// The asset catalog's icons are copies of the desktop's; a change there must reach iOS too.
    func testIconsAreTheDesktopIcons() throws {
        let apple = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
        let icons = apple.appendingPathComponent("Assets.xcassets/Icons")
        let desktop = apple.appendingPathComponent("../../desktop/assets/icons")
        let sets = try FileManager.default.contentsOfDirectory(atPath: icons.path).filter { $0.hasSuffix(".imageset") }
        XCTAssertFalse(sets.isEmpty)
        for set in sets {
            let name = String(set.dropLast(".imageset".count)) + ".svg"
            XCTAssertEqual(try Data(contentsOf: icons.appendingPathComponent(set).appendingPathComponent(name)),
                           try Data(contentsOf: desktop.appendingPathComponent(name)), name)
        }
    }

    func testMonthTitleShowsTheYearOnlyOutsideThisYear() {
        let w = words(now: "2026-11-20")
        XCTAssertEqual(w.monthTitle(at(w, "2026-11-02")).month, "November")
        XCTAssertNil(w.monthTitle(at(w, "2026-11-02")).year)
        XCTAssertEqual(w.monthTitle(at(w, "2027-01-04")).month, "January")
        XCTAssertEqual(w.monthTitle(at(w, "2027-01-04")).year, "2027")
        XCTAssertEqual(w.monthTitle(at(w, "2025-12-29")).year, "2025")
    }

    func testDateGroupsStopAtTheStartOfTheWeek() {
        let w = words(now: "2026-11-12") // Thursday
        XCTAssertEqual(w.group(at(w, "2026-11-12")), "Today")
        XCTAssertEqual(w.group(at(w, "2026-11-11")), "Yesterday")
        XCTAssertEqual(w.group(at(w, "2026-11-10")), "Tuesday")
        XCTAssertEqual(w.group(at(w, "2026-11-09")), "Monday")
        XCTAssertEqual(w.group(at(w, "2026-11-08")), "Earlier")
        let monday = words(now: "2026-11-09")
        XCTAssertEqual(monday.group(at(monday, "2026-11-08")), "Yesterday")
        XCTAssertEqual(monday.group(at(monday, "2026-11-07")), "Earlier")
    }

    func testDueChipsAreRelativeThenDatedAndOverdueBeforeToday() {
        let w = words(now: "2026-11-09")
        for (due, label, overdue) in [("2026-11-08", "Yesterday", true), ("2026-11-09T23:00:00Z", "Today", false),
                                      ("2026-11-10", "Tomorrow", false), ("2026-11-11", "Wednesday", false),
                                      ("2026-11-16", "16 Nov", false), ("2026-11-07", "7 Nov", true),
                                      ("2027-01-12", "12 Jan 2027", false), ("2025-11-12", "12 Nov 2025", true)] {
            XCTAssertEqual(w.due(due)?.label, label, due)
            XCTAssertEqual(w.due(due)?.overdue, overdue, due)
        }
        XCTAssertNil(w.due("soon"))
        XCTAssertEqual(w.dayTitle("2026-11-09"), "Today")
        XCTAssertEqual(w.dayTitle("2026-11-10"), "Tomorrow")
        XCTAssertEqual(w.dayTitle("2026-11-12"), "Thursday 12 November")
        XCTAssertEqual(w.coreDay(at(w, "2026-02-03")), "2026-02-03")
        var buddhist = Calendar(identifier: .buddhist)
        buddhist.timeZone = w.calendar.timeZone
        let thai = DateWords(now: w.now, calendar: buddhist, locale: w.locale)
        XCTAssertEqual(thai.coreDay(w.now), "2026-11-09", "core days stay Gregorian in other calendars")
        XCTAssertEqual(thai.due("2026-11-10")?.label, "Tomorrow")
    }

    func testGroupingKeepsOrderAndOnlyMergesNeighbours() {
        let groups = grouped(["a1", "a2", "b1", "a3"]) { String($0.prefix(1)) }
        XCTAssertEqual(groups.map(\.title), ["a", "b", "a"])
        XCTAssertEqual(groups.map(\.items), [["a1", "a2"], ["b1"], ["a3"]])
        // A title that comes back, as in search, still gets an id of its own for ForEach.
        XCTAssertEqual(groups.map(\.id), ["a", "b", "a #2"])
    }

    func testOnlyAFewArrivingOrLeavingRowsAnimate() {
        XCTAssertTrue(animatesRowChange(from: [1, 2, 3], to: [0, 1, 2, 3]), "new mail grows in")
        XCTAssertTrue(animatesRowChange(from: [1, 2, 3], to: [1, 3]), "a done row leaves")
        XCTAssertFalse(animatesRowChange(from: [1, 2, 3], to: [3, 2, 1]), "a reorder has nothing to show")
        XCTAssertFalse(animatesRowChange(from: [Int](), to: Array(0..<100)), "a page appears without motion")
        XCTAssertFalse(animatesRowChange(from: [1, 2], to: [1, 2]))
    }

    func testWeekBlocksShareOverlapsInLanesAsOnDesktop() {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        let day = calendar.date(from: DateComponents(year: 2026, month: 11, day: 10))!
        let at = { (hour: Double) in Int64(day.timeIntervalSince1970 + hour * 3600) }
        func event(_ from: Double, _ to: Double, allDay: Bool = false) -> AgendaItem {
            AgendaItem(day: "2026-11-10", account: "a", calendar: "c", title: "", startUnix: at(from), endUnix: at(to), allDay: allDay, location: nil, joinUrl: nil)
        }
        let events = [event(9, 10), event(9.5, 11), event(10, 10.25), event(12, 12.1), event(-2, 1), event(23, 26), event(0, 24, allDay: true), event(23.98, 24)]
        let blocks = weekBlocks(events, day: day, calendar: calendar)
        XCTAssertEqual(blocks.map(\.index), [4, 0, 1, 2, 3, 5, 7], "all-day events stay off the grid")
        XCTAssertEqual(blocks.map(\.lane), [0, 0, 1, 0, 0, 0, 1], "the 10:00 event reuses the lane the 9:00 one left")
        XCTAssertEqual(blocks.map(\.lanes), [1, 2, 2, 2, 1, 2, 2])
        XCTAssertEqual(blocks[6].end - blocks[6].start, 36, "a 23:59 event moves up to stay 36 minutes tall")
        XCTAssertEqual(blocks[0].start, 0, "yesterday's overnight event starts at the top")
        XCTAssertEqual(blocks[4].end, 12 * 60 + 36, "a six-minute event draws 36 minutes tall")
        XCTAssertEqual(blocks[5].end, 1440, "an event past midnight ends at the bottom")
        XCTAssertEqual(PlannerModel.monday(day.addingTimeInterval(5 * 86400 + 3600), calendar: calendar),
                       calendar.date(from: DateComponents(year: 2026, month: 11, day: 9)), "Sunday belongs to the week before")
    }

    func testSectionsKeepTheirStoredNames() {
        // `@AppStorage` restores these strings; renaming one would reset the user's last page.
        XCTAssertEqual(AppSection.allCases.map(\.rawValue), ["today", "mail", "calendar", "todo", "settings"])
        XCTAssertEqual(MailboxChoice.trash.mailbox, .trash)
    }

    func testTaskSwipeHasNoLaterAndDeletesOnlyAtMailsReach() {
        XCTAssertEqual(SwipeStage(dx: 120, width: 400, canDelete: true, canLater: false), .done)
        XCTAssertNil(SwipeStage(dx: -200, width: 400, canDelete: true, canLater: false))
        XCTAssertEqual(SwipeStage(dx: -240, width: 400, canDelete: true, canLater: false), .delete)
        XCTAssertEqual(SwipeStage(dx: -200, width: 400, canDelete: true), .later)
    }

    func testDueTasksCountOnlyTodayAndEarlier() {
        // Today's list also shows a few undated and future tasks; the sentence counts only the due ones, as desktop.
        func todo(_ due: String?) -> Todo { Todo(id: 1, list: 1, title: "t", due: due, done: false, fromMail: false, mailSubject: nil) }
        let tasks = [todo("2026-11-16"), todo("2026-11-17T09:00:00Z"), todo("2026-11-18"), todo(nil)]
        XCTAssertEqual(dueCount(tasks, day: "2026-11-17"), 2)
    }
}

@MainActor final class PlannerFixtureCore: LagomCore, @unchecked Sendable {
    var lists = [TodoList(id: 1, name: "Work", isDefault: false), TodoList(id: 2, name: "Private", isDefault: true)]
    var tasks: [Todo] = []
    var added: [(Int64, String, String?)] = []
    var counts = [MailCounts(account: "a", inbox: 3, later: 1), MailCounts(account: "b", inbox: 4, later: 0)]

    override func todoLists() async throws -> [TodoList] { lists }
    /// Holds the next `todos` call after its snapshot until `release()`, once.
    var holdNextTodos = false
    private(set) var held: CheckedContinuation<Void, Never>?
    func release() { held?.resume(); held = nil }
    override func todos(list: Int64, includeDone: Bool) async throws -> [Todo] {
        let snapshot = tasks.filter { $0.list == list }
        if holdNextTodos { holdNextTodos = false; await withCheckedContinuation { held = $0 } }
        return snapshot
    }
    var refuseAdd = false
    override func addTodo(list: Int64, title: String, dueDate: String?) async throws -> Todo {
        if refuseAdd { throw MailError.Failed(reason: "not found") }
        added.append((list, title, dueDate))
        let todo = Todo(id: Int64(tasks.count + 1), list: list, title: title, due: dueDate, done: false, fromMail: false, mailSubject: nil)
        tasks.append(todo)
        return todo
    }
    override func editTodo(id: Int64, title: String?, changeDue: Bool, dueDate: String?) async throws {
        guard let index = tasks.firstIndex(where: { $0.id == id }) else { throw MailError.Failed(reason: "not found") }
        if let title { tasks[index].title = title }
        if changeDue { tasks[index].due = dueDate }
    }
    override func setTodoDone(id: Int64, done: Bool) async throws {
        guard let index = tasks.firstIndex(where: { $0.id == id }) else { throw MailError.Failed(reason: "not found") }
        tasks[index].done = done
    }
    override func today(unreadLimit: UInt32) async throws -> TodaySummary {
        TodaySummary(day: "2026-11-10", events: [], tasks: tasks.filter { !$0.done }, unreadCount: 3, unread: [])
    }
    override func mailCounts() async throws -> [MailCounts] { counts }
    var mail: [Int64: Int64] = [:]
    override func todoMail(id: Int64) async throws -> Int64? { mail[id] }
    override func agenda(startDay: String, days: UInt32) async throws -> [AgendaItem] { [] }
    override func calendarConnected() async throws -> Bool { false }
    override func calendarAccessNeeded() async throws -> [MailAccount] { [] }
    override func calendarGranted() async throws -> Bool { false }
    override func tasksAccessNeeded() async throws -> [MailAccount] { [] }
    var calendars = [EventCalendar(account: "g", calendar: "primary", label: "Personal · you@gmail.com")]
    var created: [(String, String, EventForm)] = []
    var agendaLoads = 0
    override func eventCalendars() async throws -> [EventCalendar] { agendaLoads += 1; return calendars }
    override func createEvent(accountId: String, calendar: String, form: EventForm) async throws {
        if form.endUnix <= form.startUnix && !form.allDay { throw MailError.Failed(reason: "End must be later than start") }
        created.append((accountId, calendar, form))
    }
}

extension PresentationTests {
    func testNewEventDefaultsFollowTheMac() {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "Europe/Stockholm")!
        let day = calendar.date(from: DateComponents(year: 2026, month: 11, day: 20))!
        let at = { (h: Int, m: Int) in calendar.date(bySettingHour: h, minute: m, second: 0, of: day)! }
        XCTAssertEqual(PlannerModel.newEventTimes(day: day, now: at(13, 1), calendar: calendar).start, at(13, 15), "today: the next quarter hour")
        XCTAssertEqual(PlannerModel.newEventTimes(day: day, now: at(13, 0), calendar: calendar).start, at(13, 0))
        XCTAssertEqual(PlannerModel.newEventTimes(day: day, now: at(23, 50), calendar: calendar).start, at(23, 45), "never past 23:45")
        let other = PlannerModel.newEventTimes(day: day, now: at(13, 1).addingTimeInterval(86400), calendar: calendar)
        XCTAssertEqual(other.start, at(9, 0), "another day: 09:00")
        XCTAssertEqual(other.end, at(10, 0), "an hour long")
        // Clock-change days: 09:00 on the wall clock, not 540 minutes after midnight.
        for (month, day) in [(3, 29), (10, 25)] {
            let change = calendar.date(from: DateComponents(year: 2026, month: month, day: day))!
            let start = PlannerModel.newEventTimes(day: change, now: at(13, 1), calendar: calendar).start
            XCTAssertEqual(calendar.dateComponents([.hour, .minute], from: start), DateComponents(hour: 9, minute: 0), "2026-\(month)-\(day)")
        }
        let form = PlannerModel.eventForm(title: "Lunch", allDay: true, start: at(23, 30), end: at(23, 30).addingTimeInterval(3600),
                                          guests: "", description: "", meet: false, calendar: calendar)
        XCTAssertEqual([form.firstDay, form.lastDay], ["2026-11-20", "2026-11-21"], "all-day days are local dates")
        XCTAssertEqual(form.timeZone, "Europe/Stockholm")
        XCTAssertEqual(form.startUnix, Int64(at(23, 30).timeIntervalSince1970))
    }

    @MainActor func testPlannerSavesEventsThroughCoreAndReloadsTheWeek() async {
        let core = PlannerFixtureCore(noHandle: .init())
        let planner = PlannerModel()
        planner.core = core
        await planner.reloadAgenda()
        XCTAssertEqual(planner.eventCalendars.map(\.label), ["Personal · you@gmail.com"])
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        let bad = PlannerModel.eventForm(title: "Lunch", allDay: false, start: now, end: now, guests: "", description: "", meet: false)
        let refused = await planner.createEvent(bad, in: planner.eventCalendars[0])
        XCTAssertEqual(refused, "End must be later than start", "core's words reach the form")
        XCTAssertEqual(core.agendaLoads, 2, "a refused event reloads the calendars: a refused write scope hides New event")
        let good = PlannerModel.eventForm(title: "Lunch", allDay: false, start: now, end: now.addingTimeInterval(3600), guests: "", description: "", meet: true)
        let saved = await planner.createEvent(good, in: planner.eventCalendars[0])
        XCTAssertNil(saved)
        XCTAssertEqual(core.created.map { "\($0.0) \($0.1) \($0.2.title) \($0.2.meet)" }, ["g primary Lunch true"])
        XCTAssertEqual(core.agendaLoads, 3, "the week reloads at once, with Google's copy")
        core.calendars = []
        await planner.reloadAgenda()
        XCTAssertTrue(planner.eventCalendars.isEmpty, "no writable calendar hides New event")
    }
}

extension PresentationTests {
    @MainActor func testEditingPreservesSyncedDueAndBlankTitleStillSavesDate() async throws {
        let core = PlannerFixtureCore(noHandle: .init())
        let planner = PlannerModel()
        planner.core = core
        let stamp = "2026-11-12T08:00:00Z"
        let task = Todo(id: 7, list: 1, title: "Milk", due: stamp, done: false, fromMail: false, mailSubject: nil)
        core.tasks = [task]
        await planner.edit(task, title: "  Bread  ", due: DateWords().day(stamp))
        let renamed = try XCTUnwrap(planner.todos[1]?.first)
        XCTAssertEqual(renamed.title, "Bread")
        XCTAssertEqual(renamed.due, stamp)
        await planner.edit(renamed, title: "  ", due: nil)
        XCTAssertEqual(planner.todos[1]?.first?.title, "Bread")
        XCTAssertNil(planner.todos[1]?.first?.due)
    }

    @MainActor func testPlannerAddsTrimmedTasksAndSkipsBlankOnes() async {
        let core = PlannerFixtureCore(noHandle: .init())
        let planner = PlannerModel()
        planner.core = core
        await planner.reload()
        XCTAssertEqual(planner.defaultList, 2)
        XCTAssertEqual(planner.unreadCount, 3)
        XCTAssertEqual(planner.inboxLeft(account: nil), 7)
        XCTAssertEqual(planner.inboxLeft(account: "b"), 4)
        XCTAssertEqual(planner.later(account: nil), 1)
        let blank = await planner.add(title: "  \n", list: 2, due: nil)
        XCTAssertFalse(blank)
        XCTAssertTrue(core.added.isEmpty, "a blank title never reaches core")
        var calendar = Calendar.current
        calendar.timeZone = .current
        let due = calendar.date(from: DateComponents(year: 2026, month: 11, day: 12, hour: 23, minute: 30))!
        let added = await planner.add(title: "  Pack  ", list: 1, due: due)
        XCTAssertTrue(added)
        XCTAssertEqual(core.added.first?.1, "Pack")
        XCTAssertEqual(core.added.first?.2, "2026-11-12", "a late evening stays on its local day")
        XCTAssertEqual(planner.todos[1]?.map(\.title), ["Pack"])
        XCTAssertEqual(planner.today?.tasks.count, 1)
        core.refuseAdd = true
        let refused = await planner.add(title: "Call Maja", list: 1, due: nil)
        XCTAssertFalse(refused)
        XCTAssertEqual(planner.error, "Could not add “Call Maja”: not found", "the alert names a refused task")
    }

    @MainActor func testAnOlderTaskReloadThatEndsLastNeverHidesANewTask() async {
        let core = PlannerFixtureCore(noHandle: .init())
        let planner = PlannerModel()
        planner.core = core
        core.holdNextTodos = true
        let old = Task { await planner.reloadTodos() }
        while core.held == nil { await Task.yield() }
        let added = await planner.add(title: "Second", list: 1, due: nil)
        XCTAssertTrue(added)
        XCTAssertEqual(planner.todos[1]?.map(\.title), ["Second"])
        core.release()
        await old.value
        XCTAssertEqual(planner.todos[1]?.map(\.title), ["Second"], "the older snapshot must not land")
    }

    @MainActor func testPlannerTogglesThroughCoreAndReloads() async {
        let core = PlannerFixtureCore(noHandle: .init())
        core.tasks = [Todo(id: 7, list: 1, title: "Call", due: nil, done: false, fromMail: false, mailSubject: nil)]
        let planner = PlannerModel()
        planner.core = core
        await planner.reloadTodos()
        XCTAssertEqual(planner.openCount(list: 1), 1)
        XCTAssertFalse(planner.allClear)
        await planner.toggle(planner.todos[1]![0])
        XCTAssertEqual(planner.todos[1]?.first?.done, true)
        XCTAssertEqual(planner.openCount(list: 1), 0, "a done task leaves the heading count")
        XCTAssertTrue(planner.allClear, "no open task in any list")
        XCTAssertEqual(planner.today?.tasks.count, 0)
        await planner.toggle(Todo(id: 99, list: 1, title: "Gone", due: nil, done: false, fromMail: false, mailSubject: nil))
        XCTAssertNotNil(planner.error, "a refused toggle is reported")
    }

    @MainActor func testTaskMailOpensItsMessageOrSaysItIsGone() async {
        let core = PlannerFixtureCore(noHandle: .init())
        core.mail = [7: 42]
        let planner = PlannerModel()
        planner.core = core
        let task = Todo(id: 7, list: 1, title: "Reply", due: nil, done: false, fromMail: true, mailSubject: "Hello")
        let found = await planner.mail(for: task)
        XCTAssertEqual(found, 42)
        XCTAssertNil(planner.error)
        core.mail = [:]
        let gone = await planner.mail(for: task)
        XCTAssertNil(gone)
        XCTAssertEqual(planner.error, "The linked mail is no longer available.")
    }
}
