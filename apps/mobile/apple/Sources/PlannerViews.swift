import SwiftUI

/// The desktop's calm Today page: schedule, what is due, and a short look at unread mail.
struct TodayView: View {
    let planner: PlannerModel
    let model: MailModel
    let openMail: (Int64?) -> Void
    @Environment(\.horizontalSizeClass) private var width
    @State private var newTask = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 32) {
                Text(Date.now, format: .dateTime.weekday(.wide).day().month(.wide))
                    .font(.subheadline).foregroundStyle(Theme.secondary).padding(.bottom, -16)
                if width == .regular {
                    HStack(alignment: .top, spacing: 40) { schedule; tasks }
                } else {
                    schedule
                    tasks
                }
                mail
            }
            .padding(.horizontal, 20).padding(.vertical, 8)
            .frame(maxWidth: 980, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        .background(Theme.background)
        .navigationTitle("Today")
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button { newTask = true } label: { Label("New task", systemImage: "plus") }.disabled(planner.lists.isEmpty)
            }
        }
        .refreshable { await planner.reload() }
        .sheet(isPresented: $newTask) { NewTaskView(planner: planner, list: planner.defaultList, due: Date()) }
    }

    private var schedule: some View {
        VStack(alignment: .leading, spacing: 10) {
            heading("Schedule")
            let events = planner.today?.events ?? []
            if events.isEmpty {
                quiet(planner.calendarConnected ? "Nothing scheduled today." : "Your calendar shows here once a Google account is connected. Google sign-in is coming to iPhone and iPad.",
                      symbol: "calendar")
            }
            ForEach(Array(events.enumerated()), id: \.offset) { _, event in
                EventRow(event: event, accent: accountAccent(model.accounts, event.account))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var tasks: some View {
        VStack(alignment: .leading, spacing: 4) {
            heading("To do").padding(.bottom, 6)
            let tasks = planner.today?.tasks ?? []
            if tasks.isEmpty { quiet("Nothing due today.", symbol: "checkmark.circle").padding(.bottom, 6) }
            ForEach(tasks, id: \.id) { todo in
                TodoRow(todo: todo) { Task { await planner.toggle(todo) } }
            }
            Button { newTask = true } label: {
                Label("New task", systemImage: "plus").font(.body).foregroundStyle(Theme.secondary)
            }
            .buttonStyle(.plain).padding(.vertical, 8).disabled(planner.lists.isEmpty)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var mail: some View {
        VStack(alignment: .leading, spacing: 10) {
            heading("Mail")
            let unread = planner.today?.unread ?? []
            if unread.isEmpty {
                quiet(model.accounts.isEmpty ? "Add an account in Mail to see new mail here." : "No unread mail in Inbox.", symbol: "tray")
            }
            ForEach(unread, id: \.id) { row in
                Button { openMail(row.id) } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(RowPresentation(row).sender).font(.body.weight(.semibold)).lineLimit(1)
                        Text(RowPresentation(row).subject).font(.subheadline).foregroundStyle(Theme.secondary).lineLimit(1)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityHint("Opens the message")
            }
            if planner.unreadCount > 0 {
                Button { openMail(nil) } label: {
                    Label { Text("\(planner.unreadCount) unread in Inbox") + Text(" ›").foregroundStyle(Theme.secondary) } icon: { Image(systemName: "envelope") }
                        .font(.subheadline)
                }
                .buttonStyle(.plain).foregroundStyle(Theme.secondary).padding(.top, 2)
            }
        }
    }

}

private func heading(_ text: String) -> some View {
    Text(text).font(.headline).foregroundStyle(Theme.ink).accessibilityAddTraits(.isHeader)
}

private func quiet(_ text: String, symbol: String) -> some View {
    Label(text, systemImage: symbol).font(.subheadline).foregroundStyle(Theme.secondary)
}

/// One schedule line: time, the account's bar, the title, and a meeting link when there is one.
struct EventRow: View {
    let event: AgendaItem
    var accent: Color?
    var now = Date()

    var body: some View {
        let start = Date(timeIntervalSince1970: TimeInterval(event.startUnix))
        let end = Date(timeIntervalSince1970: TimeInterval(event.endUnix))
        let past = !event.allDay && end < now
        HStack(alignment: .center, spacing: 12) {
            Group {
                if event.allDay {
                    Image(systemName: "sun.max").foregroundStyle(Theme.later).accessibilityLabel("All day")
                } else {
                    Text(start, format: .dateTime.hour().minute())
                }
            }
            .font(.subheadline.monospacedDigit()).foregroundStyle(Theme.secondary)
            .frame(minWidth: 52, alignment: .leading)
            RoundedRectangle(cornerRadius: 1.5).fill(accent ?? Theme.accent).frame(width: 3).frame(minHeight: 22)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) {
                Text(event.title.isEmpty ? "(No title)" : event.title).font(.body).foregroundStyle(past ? Theme.secondary : Theme.ink)
                if let location = event.location, !location.isEmpty {
                    Text(location).font(.footnote).foregroundStyle(Theme.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 0)
            if let link = event.joinUrl.flatMap(URL.init(string:)), ["https", "http"].contains(link.scheme?.lowercased() ?? "") {
                Link(destination: link) { Image(systemName: "video").font(.body) }
                    .accessibilityLabel("Join meeting")
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
        .accessibilityValue(event.allDay ? "" : (start..<max(start, end)).formatted(.interval.hour().minute()))
    }
}

/// A task: a completion circle, its title, and the due chip in the danger colour once overdue.
struct TodoRow: View {
    let todo: Todo
    let toggle: () -> Void
    var words = DateWords()

    var body: some View {
        HStack(spacing: 12) {
            Button(action: toggle) {
                Image(systemName: todo.done ? "checkmark.circle.fill" : "circle")
                    .font(.title3).foregroundStyle(todo.done ? Theme.done : Theme.secondary)
                    .frame(minWidth: 44, minHeight: 44).contentShape(Rectangle())
            }
            .buttonStyle(.plain).padding(.horizontal, -10)
            .accessibilityLabel(todo.done ? "Mark as not done" : "Mark as done")
            Text(todo.title).font(.body)
                .strikethrough(todo.done).foregroundStyle(todo.done ? Theme.secondary : Theme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
            if todo.fromMail {
                Image(systemName: "envelope").font(.footnote).foregroundStyle(Theme.secondary).accessibilityLabel("From mail")
            }
            if !todo.done, let due = todo.due.flatMap(words.due) {
                Text(due.label).font(.footnote)
                    .foregroundStyle(due.overdue ? Theme.danger : Theme.secondary)
                    .padding(.horizontal, 6).padding(.vertical, 3)
                    .background(due.overdue ? Theme.danger.opacity(0.1) : Theme.subtle, in: RoundedRectangle(cornerRadius: Theme.Radius.key))
                    .accessibilityLabel(due.overdue ? "Overdue, due \(due.label)" : "Due \(due.label)")
            }
        }
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
        List {
            ForEach(planner.lists, id: \.id) { list in
                let all = planner.todos[list.id] ?? []
                let done = all.filter(\.done)
                Section {
                    ForEach(all.filter { !$0.done }, id: \.id) { row($0) }
                    Button { adding = list.id } label: {
                        Label("Add task", systemImage: "plus").foregroundStyle(Theme.secondary)
                    }
                    if !done.isEmpty {
                        DisclosureGroup("Completed (\(done.count))", isExpanded: Binding(
                            get: { showDone.contains(list.id) },
                            set: { if $0 { showDone.insert(list.id) } else { showDone.remove(list.id) } })) {
                            ForEach(done, id: \.id) { row($0) }
                        }
                        .foregroundStyle(Theme.secondary)
                    }
                } header: {
                    Text(list.name).font(.headline).foregroundStyle(Theme.ink).textCase(nil)
                }
                .listRowBackground(Theme.background)
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden).background(Theme.background)
        .overlay {
            if planner.lists.isEmpty {
                ContentUnavailableView("No lists", systemImage: "checkmark.square", description: Text("Add a list to start."))
            }
        }
        .navigationTitle("Todo")
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Menu {
                    Button("New task", systemImage: "plus") { adding = planner.defaultList }.disabled(planner.lists.isEmpty)
                    Button("New list", systemImage: "list.bullet") { listName = ""; newList = true }
                } label: { Label("Add", systemImage: "plus") }
            }
        }
        .refreshable { await planner.reloadTodos() }
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
            .listRowBackground(Theme.background)
            .swipeActions {
                Button(role: .destructive) { Task { await planner.delete(todo) } } label: { Label("Delete", systemImage: "trash") }
                    .tint(Theme.danger)
            }
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
        NavigationStack {
            Form {
                TextField("Task", text: $title).focused($focused).submitLabel(.done).onSubmit(save)
                if planner.lists.count > 1 {
                    Picker("List", selection: $list) {
                        ForEach(planner.lists, id: \.id) { Text($0.name).tag(Optional($0.id)) }
                    }
                }
                Toggle("Due date", isOn: Binding(get: { due != nil }, set: { due = $0 ? (due ?? Date()) : nil }))
                if let date = due {
                    DatePicker("Due", selection: Binding(get: { date }, set: { due = $0 }), displayedComponents: .date)
                }
            }
            .navigationTitle("New task").navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add", action: save).disabled(saving || list == nil || title.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
            .onAppear { focused = true }
        }
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

/// Read-only agenda of cached events; core syncs Google Calendar on desktop.
struct CalendarView: View {
    let planner: PlannerModel
    let model: MailModel
    private let words = DateWords()

    var body: some View {
        List {
            ForEach(grouped(planner.agenda, by: \.day)) { day in
                Section {
                    ForEach(Array(day.items.enumerated()), id: \.offset) { _, event in
                        EventRow(event: event, accent: accountAccent(model.accounts, event.account)).listRowBackground(Theme.background)
                    }
                } header: {
                    Text(words.dayTitle(day.title)).font(.headline).foregroundStyle(Theme.ink).textCase(nil)
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden).background(Theme.background)
        .overlay {
            if planner.agenda.isEmpty {
                if planner.calendarConnected {
                    ContentUnavailableView("Nothing scheduled", systemImage: "calendar",
                                           description: Text("No events in the next two weeks."))
                } else {
                    ContentUnavailableView("Calendar needs a Google account", systemImage: "calendar",
                                           description: Text("Lagom shows your Google Calendar here. Google sign-in is coming to iPhone and iPad; mail accounts with a password have no calendar."))
                }
            }
        }
        .navigationTitle("Calendar")
        .refreshable { await planner.reloadAgenda() }
    }

}
