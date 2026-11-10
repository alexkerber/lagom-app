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
    var calendarConnected = false
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
            self.agenda = agenda
        } catch { self.error = error.localizedDescription }
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

    func toggle(_ todo: Todo) async {
        guard let core else { return }
        do { try await core.setTodoDone(id: todo.id, done: !todo.done) } catch { self.error = error.localizedDescription }
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
