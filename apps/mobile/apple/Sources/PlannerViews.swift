import SwiftUI

/// Page padding: the desktop's 24 on iPad, 16 at phone width.
private struct PagePadding: ViewModifier {
    @Environment(\.horizontalSizeClass) private var width
    func body(content: Content) -> some View { content.padding(.horizontal, width == .regular ? 24 : 16) }
}

/// The desktop's calm Today page: schedule, what is due, and a short look at unread mail.
struct TodayView: View {
    let planner: PlannerModel
    let model: MailModel
    let openMail: (Int64?) -> Void
    @Environment(\.horizontalSizeClass) private var width
    @State private var newTask = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            PageTitle(title: "Today") {
                Text(Date.now, format: .dateTime.weekday(.wide).day().month(.wide))
                    .lagomFont(.small).foregroundStyle(Theme.secondary)
            }
            .modifier(PagePadding()).padding(.top, 8)
            ScrollView {
                VStack(alignment: .leading, spacing: 32) {
                    if width == .regular {
                        HStack(alignment: .top, spacing: 32) { schedule; tasks }
                    } else {
                        schedule
                        tasks
                    }
                    mail
                }
                .modifier(PagePadding()).padding(.vertical, 16)
            }
            .refreshable { await planner.reload() }
            if planner.unreadCount > 0 {
                Button { openMail(nil) } label: {
                    HStack(spacing: 8) {
                        Icon("mail")
                        Text("\(planner.unreadCount) unread in Inbox").lagomFont(.base)
                        Icon("chevron-right")
                    }
                    .foregroundStyle(Theme.secondary).frame(minHeight: 44).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .modifier(PagePadding()).padding(.bottom, 4)
            }
        }
        .background(Theme.background)
        .sheet(isPresented: $newTask) { NewTaskView(planner: planner, list: planner.defaultList, due: Date()) }
    }

    private var schedule: some View {
        let events = planner.today?.events ?? []
        let now = Date()
        // The now line sits before the first timed event still to start, as on desktop.
        let nowIndex = events.firstIndex { !$0.allDay && Date(timeIntervalSince1970: TimeInterval($0.startUnix)) > now } ?? events.count
        return VStack(alignment: .leading, spacing: 4) {
            Heading(title: "Schedule").padding(.bottom, 8)
            if events.isEmpty {
                quiet(planner.calendarConnected ? "Nothing scheduled today." : "Your calendar shows here once a Google account is connected. Google sign-in is coming to iPhone and iPad.",
                      icon: "calendar")
            } else {
                ForEach(Array(events.enumerated()), id: \.offset) { index, event in
                    if index == nowIndex { NowLine(now: now) }
                    EventRow(event: event, accent: accountAccent(model.accounts, event.account), now: now)
                }
                if nowIndex == events.count { NowLine(now: now) }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var tasks: some View {
        VStack(alignment: .leading, spacing: 0) {
            Heading(title: "To do").padding(.bottom, 8)
            let tasks = planner.today?.tasks ?? []
            if tasks.isEmpty { quiet("Nothing due today.", icon: "todo").padding(.bottom, 6) }
            ForEach(tasks, id: \.id) { todo in
                TodoRow(todo: todo) { Task { await planner.toggle(todo) } }
            }
            AddRow(title: "New task") { newTask = true }.disabled(planner.lists.isEmpty)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var mail: some View {
        VStack(alignment: .leading, spacing: 4) {
            Heading(title: "Mail").padding(.bottom, 8)
            let unread = planner.today?.unread ?? []
            if unread.isEmpty {
                quiet(model.accounts.isEmpty ? "Add an account in Mail to see new mail here." : "No unread mail in Inbox.", icon: "inbox")
            }
            ForEach(unread, id: \.id) { row in
                Button { openMail(row.id) } label: {
                    HStack(spacing: 10) {
                        Circle().fill(Theme.accent).frame(width: 7, height: 7)
                        Text(RowPresentation(row).sender).lagomFont(.base, .semibold).lineLimit(1)
                        Text(RowPresentation(row).subject).lagomFont(.base).foregroundStyle(Theme.secondary).lineLimit(1)
                        Spacer(minLength: 0)
                    }
                    .frame(minHeight: 44).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityHint("Opens the message")
            }
        }
    }
}

private func quiet(_ text: String, icon: String) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: 8) {
        Icon(icon).alignmentGuide(.firstTextBaseline) { $0[.bottom] - 3 }
        Text(text).lagomFont(.snippet)
    }
    .foregroundStyle(Theme.secondary)
}

/// "+ New task", "+ Add task": a quiet line that adds.
private struct AddRow: View {
    let title: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Icon("plus").frame(width: 22)
                Text(title).lagomFont(.base)
            }
            .foregroundStyle(Theme.secondary).padding(.horizontal, 8)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// The current time in gold across the schedule.
private struct NowLine: View {
    let now: Date

    var body: some View {
        HStack(spacing: 10) {
            Text(now, format: .dateTime.hour().minute()).lagomFont(.small).monospacedDigit().fixedSize().frame(minWidth: 52, alignment: .leading)
            Capsule().frame(height: 2)
        }
        .foregroundStyle(Theme.now)
        .frame(minHeight: 28)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Now, \(now.formatted(.dateTime.hour().minute()))")
    }
}

/// One schedule line, desktop `agenda_view`: time, the account's bar, the title, and a meeting link.
struct EventRow: View {
    let event: AgendaItem
    var accent: Color?
    var now = Date()

    var body: some View {
        let start = Date(timeIntervalSince1970: TimeInterval(event.startUnix))
        let end = Date(timeIntervalSince1970: TimeInterval(event.endUnix))
        let past = !event.allDay && end < now
        HStack(alignment: .center, spacing: 10) {
            if event.allDay {
                Icon("sun").foregroundStyle(Theme.secondary).frame(width: 52, alignment: .trailing)
                Circle().fill(accent ?? Theme.accent).frame(width: 5, height: 5)
            } else {
                Text(start, format: .dateTime.hour().minute()).lagomFont(.small).monospacedDigit()
                    .foregroundStyle(Theme.secondary).fixedSize().frame(minWidth: 52, alignment: .leading)
                RoundedRectangle(cornerRadius: 1).fill(accent ?? Theme.accent).frame(width: 2, height: 28)
            }
            VStack(alignment: .leading, spacing: 1) {
                Text(event.title.isEmpty ? "(No title)" : event.title).lagomFont(.base).lineLimit(1)
                    .foregroundStyle(past ? Theme.secondary : Theme.ink)
                if let location = event.location, !location.isEmpty {
                    Text(location).lagomFont(.small).foregroundStyle(Theme.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 0)
            if let link = event.joinUrl.flatMap(URL.init(string:)), ["https", "http"].contains(link.scheme?.lowercased() ?? "") {
                Link(destination: link) { Icon("video").foregroundStyle(Theme.secondary).frame(minWidth: 44, minHeight: 44) }
                    .accessibilityLabel("Join meeting")
            }
        }
        .frame(minHeight: 44)
        .accessibilityElement(children: .combine)
        .accessibilityLabel([event.allDay ? "All day" : nil, event.title.isEmpty ? "(No title)" : event.title, event.location]
            .compactMap { $0?.isEmpty == false ? $0 : nil }.joined(separator: ", "))
        .accessibilityValue(event.allDay ? "" : (start..<max(start, end)).formatted(.interval.hour().minute()))
    }
}

/// A task, desktop `task_row`: a completion circle, its title, a mail mark, and the due chip.
struct TodoRow: View {
    let todo: Todo
    let toggle: () -> Void
    var words = DateWords()

    var body: some View {
        HStack(spacing: 10) {
            Button(action: toggle) {
                Circle().strokeBorder(Theme.secondary, lineWidth: 1)
                    .frame(width: 21, height: 21)
                    .overlay { if todo.done { Icon("done", size: 16).foregroundStyle(Theme.secondary) } }
                    .frame(width: 44, height: 44).contentShape(Rectangle())
            }
            .buttonStyle(.plain).padding(.horizontal, -11)
            .accessibilityLabel(todo.done ? "Mark as not done" : "Mark as done")
            Text(todo.title).lagomFont(.base)
                .strikethrough(todo.done).foregroundStyle(todo.done ? Theme.secondary : Theme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
            if todo.fromMail {
                Icon("mail", label: "From mail").foregroundStyle(Theme.secondary)
            }
            if !todo.done, let due = todo.due.flatMap(words.due) {
                Text(due.label).lagomFont(.small)
                    .foregroundStyle(due.overdue ? Theme.danger : Theme.secondary)
                    .padding(.horizontal, 7).padding(.vertical, 3)
                    .background(due.overdue ? Theme.danger.opacity(0.12) : Theme.subtle, in: RoundedRectangle(cornerRadius: Theme.Radius.key))
                    .accessibilityLabel(due.overdue ? "Overdue, due \(due.label)" : "Due \(due.label)")
            }
        }
        .padding(.horizontal, 8)
        .frame(minHeight: 46)
        .accessibilityElement(children: .contain)
    }
}

/// Local task lists, as on the desktop's Todo page.
struct TodoView: View {
    let planner: PlannerModel
    @State private var adding: Int64?
    @State private var newList = false
    @State private var listName = ""
    @State private var showDone: Set<Int64> = []

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            PageTitle(title: "Todo") {
                Menu {
                    Button("New task") { adding = planner.defaultList }.disabled(planner.lists.isEmpty)
                    Button("New list") { listName = ""; newList = true }
                } label: { Icon("plus") }
                    .menuStyle(.button).buttonStyle(IconButtonStyle())
                    .accessibilityLabel("Add")
                    .padding(.trailing, -6)
            }
            .modifier(PagePadding()).padding(.top, 8)
            List {
                ForEach(planner.lists, id: \.id) { list in
                    let all = planner.todos[list.id] ?? []
                    let done = all.filter(\.done)
                    Group {
                        Heading(title: list.name).padding(.top, 16).padding(.bottom, 4)
                        ForEach(all.filter { !$0.done }, id: \.id) { row($0) }
                        AddRow(title: "Add task") { adding = list.id }
                        if !done.isEmpty {
                            let open = showDone.contains(list.id)
                            Button {
                                if open { showDone.remove(list.id) } else { showDone.insert(list.id) }
                            } label: {
                                HStack(spacing: 8) {
                                    Icon(open ? "chevron" : "chevron-right")
                                    Text("Completed (\(done.count))").lagomFont(.base)
                                }
                                .foregroundStyle(Theme.secondary)
                                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading).contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityValue(open ? "Expanded" : "Collapsed")
                            if open { ForEach(done, id: \.id) { row($0) } }
                        }
                    }
                    .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
                    .listRowSeparator(.hidden)
                    .listRowBackground(Color.clear)
                }
            }
            .listStyle(.plain)
            .environment(\.defaultMinListRowHeight, 0)
            .scrollContentBackground(.hidden)
            .modifier(PagePadding())
            .overlay {
                if planner.lists.isEmpty { EmptyState(icon: "todo", title: "No lists", detail: "Add a list to start.") }
            }
            .refreshable { await planner.reloadTodos() }
        }
        .background(Theme.background)
        .sheet(item: Binding(get: { adding.map(ListID.init) }, set: { adding = $0?.id })) { list in
            NewTaskView(planner: planner, list: list.id, due: nil)
        }
        .alert("New list", isPresented: $newList) {
            TextField("Name", text: $listName)
            Button("Cancel", role: .cancel) {}
            Button("Add") { Task { await planner.addList(name: listName) } }
        }
    }

    private func row(_ todo: Todo) -> some View {
        TodoRow(todo: todo) { Task { await planner.toggle(todo) } }
            .swipeActions {
                Button(role: .destructive) { Task { await planner.delete(todo) } } label: { Label("Delete", image: "trash") }
                    .tint(Theme.danger)
            }
            .accessibilityAction(named: "Delete") { Task { await planner.delete(todo) } }
    }
}

private struct ListID: Identifiable { let id: Int64 }

struct NewTaskView: View {
    let planner: PlannerModel
    @State var list: Int64?
    @State var due: Date?
    @State private var title = ""
    @State private var saving = false
    @Environment(\.dismiss) private var dismiss
    @FocusState private var focused: Bool

    var body: some View {
        SheetScaffold(title: "New task", cancel: { dismiss() }) {
            VStack(spacing: 0) {
                FieldRow(label: "Task") {
                    TextField("", text: $title).focused($focused).submitLabel(.done).onSubmit(save).accessibilityLabel("Task")
                }
                if planner.lists.count > 1 {
                    FieldRow(label: "List") {
                        Picker("List", selection: $list) {
                            ForEach(planner.lists, id: \.id) { Text($0.name).tag(Optional($0.id)) }
                        }
                        .labelsHidden().tint(Theme.ink)
                    }
                }
                FieldRow(label: "Due") {
                    HStack {
                        if let date = due {
                            DatePicker("Due", selection: Binding(get: { date }, set: { due = $0 }), displayedComponents: .date).labelsHidden()
                        }
                        Spacer()
                        Toggle("Due date", isOn: Binding(get: { due != nil }, set: { due = $0 ? (due ?? Date()) : nil })).labelsHidden()
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16)
        } footer: {
            HouseButton(title: "Add task", icon: "plus", primary: true, action: save)
                .disabled(saving || list == nil || title.trimmingCharacters(in: .whitespaces).isEmpty)
            Spacer(minLength: 0)
        }
        .onAppear { focused = true }
        .presentationDetents([.medium, .large])
    }

    private func save() {
        guard let list, !saving else { return }
        saving = true
        Task {
            if await planner.add(title: title, list: list, due: due) { dismiss() }
            saving = false
        }
    }
}

/// Cached events by day under the desktop's month title; each day headed as the desktop week's columns.
struct CalendarView: View {
    let planner: PlannerModel
    let model: MailModel
    private let words = DateWords()

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Text(Date.now, format: .dateTime.month(.wide)).lagomFont(.subject, .bold)
                Text(Date.now, format: .dateTime.year()).lagomFont(.subject).foregroundStyle(Theme.secondary)
            }
            .accessibilityElement(children: .combine).accessibilityAddTraits(.isHeader)
            .frame(minHeight: 44).modifier(PagePadding()).padding(.top, 8)
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(grouped(planner.agenda, by: \.day)) { day in
                        DayHeader(title: day.title).padding(.top, 14).padding(.bottom, 2)
                        ForEach(Array(day.items.enumerated()), id: \.offset) { _, event in
                            EventRow(event: event, accent: accountAccent(model.accounts, event.account))
                        }
                    }
                }
                .modifier(PagePadding()).padding(.bottom, 16)
            }
            .refreshable { await planner.reloadAgenda() }
            .overlay {
                if planner.agenda.isEmpty {
                    if planner.calendarConnected {
                        EmptyState(icon: "calendar", title: "Nothing scheduled", detail: "No events in the next two weeks.")
                    } else {
                        EmptyState(icon: "calendar", title: "Calendar needs a Google account",
                                   detail: "lagom shows your Google Calendar here. Google sign-in is coming to iPhone and iPad; mail accounts with a password have no calendar.")
                    }
                }
            }
        }
        .background(Theme.background)
    }
}

/// "Mon 9" as the desktop week's column heads, today's number in a filled accent circle.
private struct DayHeader: View {
    let title: String

    var body: some View {
        let day = DateWords().day(title) ?? .now
        let today = Calendar.current.isDateInToday(day)
        HStack(spacing: 8) {
            Text(day, format: .dateTime.weekday(.abbreviated)).lagomFont(.base).foregroundStyle(Theme.secondary)
            Text(day, format: .dateTime.day()).lagomFont(.subject)
                .foregroundStyle(today ? Theme.accentInk : Theme.ink)
                .frame(minWidth: 38, minHeight: 38)
                .background(today ? Theme.accent : .clear, in: Circle())
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(DateWords().dayTitle(title))
        .accessibilityAddTraits(.isHeader)
    }
}
