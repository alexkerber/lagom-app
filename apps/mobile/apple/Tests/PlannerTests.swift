import XCTest

final class PresentationTests: XCTestCase {
    private func words(now: String) -> DateWords {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        let base = DateWords(now: Date(), calendar: calendar, locale: Locale(identifier: "en_GB"))
        return DateWords(now: base.day(now)!.addingTimeInterval(10 * 3600), calendar: calendar, locale: base.locale)
    }
    private func at(_ words: DateWords, _ day: String, _ hour: Double = 9.7) -> Date { words.day(day)!.addingTimeInterval(hour * 3600) }

    func testListTimeFollowsTheDesktopDateColumn() {
        let w = words(now: "2026-11-10")
        XCTAssertEqual(w.listTime(at(w, "2026-11-10", 9.7)), "09:42")
        XCTAssertEqual(w.listTime(at(w, "2026-11-09")), "Yesterday")
        XCTAssertEqual(w.listTime(at(w, "2026-11-06")), "Fri")
        XCTAssertEqual(w.listTime(at(w, "2026-10-26")), "26 Oct")
        XCTAssertEqual(w.listTime(at(w, "2025-10-01")), "Oct 2025")
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
    }

    func testSectionsKeepTheirStoredNames() {
        // `@AppStorage` restores these strings; renaming one would reset the user's last page.
        XCTAssertEqual(AppSection.allCases.map(\.rawValue), ["today", "mail", "calendar", "todo"])
        XCTAssertEqual(MailboxChoice.trash.mailbox, .trash)
    }
}

@MainActor final class PlannerFixtureCore: LagomCore, @unchecked Sendable {
    var lists = [TodoList(id: 1, name: "Work", isDefault: false), TodoList(id: 2, name: "Private", isDefault: true)]
    var tasks: [Todo] = []
    var added: [(Int64, String, String?)] = []
    var counts = [MailCounts(account: "a", inbox: 3, later: 1), MailCounts(account: "b", inbox: 4, later: 0)]

    override func todoLists() async throws -> [TodoList] { lists }
    override func todos(list: Int64, includeDone: Bool) async throws -> [Todo] { tasks.filter { $0.list == list } }
    override func addTodo(list: Int64, title: String, dueDate: String?) async throws -> Todo {
        added.append((list, title, dueDate))
        let todo = Todo(id: Int64(tasks.count + 1), list: list, title: title, due: dueDate, done: false, fromMail: false)
        tasks.append(todo)
        return todo
    }
    override func setTodoDone(id: Int64, done: Bool) async throws {
        guard let index = tasks.firstIndex(where: { $0.id == id }) else { throw MailError.Failed(message: "not found") }
        tasks[index].done = done
    }
    override func today(unreadLimit: UInt32) async throws -> TodaySummary {
        TodaySummary(day: "2026-11-10", events: [], tasks: tasks.filter { !$0.done }, unreadCount: 3, unread: [])
    }
    override func mailCounts() async throws -> [MailCounts] { counts }
    override func agenda(startDay: String, days: UInt32) async throws -> [AgendaItem] { [] }
    override func calendarConnected() async throws -> Bool { false }
}

extension PresentationTests {
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
    }

    @MainActor func testPlannerTogglesThroughCoreAndReloads() async {
        let core = PlannerFixtureCore(noHandle: .init())
        core.tasks = [Todo(id: 7, list: 1, title: "Call", due: nil, done: false, fromMail: false)]
        let planner = PlannerModel()
        planner.core = core
        await planner.reloadTodos()
        await planner.toggle(planner.todos[1]![0])
        XCTAssertEqual(planner.todos[1]?.first?.done, true)
        XCTAssertEqual(planner.today?.tasks.count, 0)
        await planner.toggle(Todo(id: 99, list: 1, title: "Gone", due: nil, done: false, fromMail: false))
        XCTAssertNotNil(planner.error, "a refused toggle is reported")
    }
}
