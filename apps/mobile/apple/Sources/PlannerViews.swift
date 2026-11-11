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
/// iPad shows the desktop's week grid instead.
struct CalendarView: View {
    let planner: PlannerModel
    let model: MailModel
    @Environment(\.horizontalSizeClass) private var width
    private let words = DateWords()

    var body: some View {
        // A landscape Pro Max is regular width too; only iPad gets the grid.
        if width == .regular && UIDevice.current.userInterfaceIdiom == .pad { WeekGrid(planner: planner, model: model) } else { list }
    }

    private var list: some View {
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

/// The desktop week (`week::week_view`): day columns on an hour grid, all-day events above, the now line across.
private struct WeekGrid: View {
    let planner: PlannerModel
    let model: MailModel
    private static let hour: CGFloat = 60
    private static let gutter: CGFloat = 48
    private let calendar = Calendar.current
    @State private var position = ScrollPosition(edge: .top)
    @State private var allDayHeight: CGFloat = 0

    var body: some View {
        let days = (0..<7).compactMap { calendar.date(byAdding: .day, value: $0, to: planner.weekStart) }
        let byDay = Dictionary(grouping: planner.week, by: \.day)
        let words = DateWords()
        VStack(alignment: .leading, spacing: 8) {
            header
            if !planner.calendarConnected && planner.week.isEmpty {
                EmptyState(icon: "calendar", title: "Calendar needs a Google account",
                           detail: "lagom shows your Google Calendar here. Google sign-in is coming to iPhone and iPad; mail accounts with a password have no calendar.")
                    .frame(maxHeight: .infinity)
            } else {
                HStack(spacing: 0) {
                    Color.clear.frame(width: Self.gutter, height: 1)
                    ForEach(days, id: \.self) { WeekDayHead(day: $0).frame(maxWidth: .infinity, alignment: .leading) }
                }
                let allDay = days.map { day in (byDay[words.coreDay(day)] ?? []).filter(\.allDay) }
                if allDay.contains(where: { !$0.isEmpty }) {
                    let row = HStack(alignment: .top, spacing: 0) {
                        Text("All day").lagomFont(.small).foregroundStyle(Theme.secondary).frame(width: Self.gutter, alignment: .leading)
                        ForEach(allDay.indices, id: \.self) { i in
                            VStack(spacing: 3) {
                                ForEach(Array(zip(eventKeys(allDay[i]), allDay[i])), id: \.0) { _, event in
                                    WeekEvent(event: event, accent: accountAccent(model.accounts, event.account), compact: true)
                                }
                            }
                            .padding(.horizontal, 2).frame(maxWidth: .infinity, alignment: .top)
                        }
                    }
                    // Many all-day events scroll in a band, as desktop's 150 px strip, so the hours stay visible.
                    ScrollView { row.onGeometryChange(for: CGFloat.self) { $0.size.height } action: { allDayHeight = $0 } }
                        .frame(height: min(allDayHeight, 110)).scrollBounceBehavior(.basedOnSize)
                }
                grid(days: days, byDay: byDay)
            }
        }
        .padding(.horizontal, 24).padding(.top, 8)
        .background(Theme.background)
        .task { await planner.reloadWeek() }
    }

    private var header: some View {
        // The month a week belongs to is its Thursday's, as ISO weeks count.
        let thursday = calendar.date(byAdding: .day, value: 3, to: planner.weekStart) ?? planner.weekStart
        return HStack(spacing: 8) {
            HStack(spacing: 6) {
                Text(thursday, format: .dateTime.month(.wide)).lagomFont(.subject, .bold)
                Text(thursday, format: .dateTime.year()).lagomFont(.subject).foregroundStyle(Theme.secondary)
            }
            .accessibilityElement(children: .combine).accessibilityAddTraits(.isHeader)
            Spacer(minLength: 8)
            IconButton(title: "Previous week", icon: "chevron-left") { Task { await planner.showWeek(-1) } }
            Button("Today") { Task { await planner.showWeek(0) } }.buttonStyle(HouseButtonStyle())
            IconButton(title: "Next week", icon: "chevron-right") { Task { await planner.showWeek(1) } }
        }
        .frame(minHeight: 44)
    }

    private func grid(days: [Date], byDay: [String: [AgendaItem]]) -> some View {
        let words = DateWords()
        let blocks = days.map { weekBlocks(byDay[words.coreDay($0)] ?? [], day: $0, calendar: calendar) }
        // Opens at the week's first event, or 08:00 on an empty week.
        let first = blocks.joined().map(\.start).min().map { max(0, Int($0 / 60) - 1) } ?? 8
        return ScrollView {
                HStack(alignment: .top, spacing: 0) {
                    VStack(spacing: 0) {
                        ForEach(0..<24, id: \.self) { hour in
                            HStack(alignment: .firstTextBaseline, spacing: 1) {
                                Text(String(format: "%02d", hour)).lagomFont(.base).foregroundStyle(Theme.ink)
                                Text("00").lagomFont(.small).foregroundStyle(Theme.secondary)
                            }
                            .monospacedDigit().padding(.trailing, 8)
                            .frame(width: Self.gutter, height: Self.hour, alignment: .topTrailing).offset(y: -8)
                        }
                    }
                    .accessibilityHidden(true)
                    ForEach(days.indices, id: \.self) { i in
                        column(day: days[i], events: byDay[words.coreDay(days[i])] ?? [], blocks: blocks[i])
                    }
                }
                .padding(.top, 12)
                .overlay(alignment: .topLeading) { NowLine24(days: days, hour: Self.hour, gutter: Self.gutter).padding(.top, 12) }
        }
        .scrollPosition($position)
        .task(id: "\(planner.weekStart) \(first)") { position.scrollTo(y: CGFloat(first) * Self.hour) }
        .refreshable { await planner.reloadAgenda() }
    }

    private func column(day: Date, events: [AgendaItem], blocks: [WeekBlock]) -> some View {
        let weekend = (calendar.component(.weekday, from: day) + 5) % 7 >= 5
        let fill = calendar.isDateInToday(day) ? Theme.accent.opacity(0.06) : weekend ? Theme.subtle : Theme.background
        let keys = eventKeys(events)
        return GeometryReader { geo in
            ZStack(alignment: .topLeading) {
                fill
                ForEach(0..<24, id: \.self) { hour in
                    Rectangle().fill(Theme.line).frame(height: 1).offset(y: CGFloat(hour) * Self.hour)
                }
                Rectangle().fill(Theme.line).frame(width: 1)
                // Keyed by the event, not its position, so an open popover stays on its event across a refresh.
                ForEach(Array(zip(blocks.map { keys[$0.index] }, blocks)), id: \.0) { _, block in
                    let width = (geo.size.width - 3) / CGFloat(block.lanes)
                    WeekEvent(event: events[block.index], accent: accountAccent(model.accounts, events[block.index].account),
                              compact: block.end - block.start < 45)
                        .frame(width: width - 2, height: CGFloat(block.end - block.start) * Self.hour / 60 - 2)
                        .offset(x: 3 + CGFloat(block.lane) * width, y: CGFloat(block.start) * Self.hour / 60 + 1)
                }
            }
        }
        .frame(maxWidth: .infinity).frame(height: 24 * Self.hour)
    }
}

/// ForEach keys from what an event is, not where it sits; identical copies are numbered so keys stay unique.
private func eventKeys(_ events: [AgendaItem]) -> [String] {
    var seen: [String: Int] = [:]
    return events.map { e in
        let key = "\(e.account)|\(e.calendar)|\(e.startUnix)|\(e.endUnix)|\(e.title)"
        seen[key, default: 0] += 1
        return "\(key)#\(seen[key]!)"
    }
}

/// "Mon 9", weekends quieter, today in the accent circle.
private struct WeekDayHead: View {
    let day: Date

    var body: some View {
        let today = Calendar.current.isDateInToday(day)
        let weekend = (Calendar.current.component(.weekday, from: day) + 5) % 7 >= 5
        HStack(spacing: 6) {
            Text(day, format: .dateTime.weekday(.abbreviated)).lagomFont(.base).foregroundStyle(Theme.secondary)
            Text(day, format: .dateTime.day()).lagomFont(.title)
                .foregroundStyle(today ? Theme.accentInk : weekend ? Theme.secondary : Theme.ink)
                .frame(minWidth: 32, minHeight: 32)
                .background(today ? Theme.accent : .clear, in: Circle())
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(day.formatted(.dateTime.weekday(.wide).day().month(.wide)))
        .accessibilityAddTraits(.isHeader)
    }
}

/// An event on the grid, desktop `event_block`: the account's tint and bar, the title, then the time. Tap shows its details.
private struct WeekEvent: View {
    let event: AgendaItem
    let accent: Color?
    let compact: Bool
    @State private var shown = false

    var body: some View {
        let tint = accent ?? Theme.accent
        let start = Date(timeIntervalSince1970: TimeInterval(event.startUnix))
        let end = Date(timeIntervalSince1970: TimeInterval(event.endUnix))
        let title = event.title.isEmpty ? "(No title)" : event.title
        Button { shown = true } label: {
            VStack(alignment: .leading, spacing: 1) {
                Text(compact && !event.allDay ? "\(title) · \(start.formatted(.dateTime.hour().minute()))" : title)
                    .lagomFont(.small, .medium).foregroundStyle(Theme.ink).lineLimit(compact ? 1 : 2)
                if !compact && !event.allDay {
                    Text(start.formatted(.dateTime.hour().minute())).lagomFont(.small).monospacedDigit().foregroundStyle(Theme.secondary).lineLimit(1)
                }
            }
            .padding(.horizontal, 5).padding(.vertical, 2)
            .frame(maxWidth: .infinity, maxHeight: event.allDay ? nil : .infinity, alignment: .topLeading)
            .background(tint.opacity(0.14), in: RoundedRectangle(cornerRadius: 4))
            .overlay(alignment: .leading) { Rectangle().fill(tint).frame(width: 2) }
            .clipShape(RoundedRectangle(cornerRadius: 4))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel([event.allDay ? "All day" : nil, title, event.location].compactMap { $0?.isEmpty == false ? $0 : nil }.joined(separator: ", "))
        .accessibilityValue(event.allDay ? "" : (start..<max(start, end)).formatted(.interval.hour().minute()))
        .popover(isPresented: $shown) {
            VStack(alignment: .leading, spacing: 12) {
                Text(title).lagomFont(.title, .semibold)
                Text(event.allDay ? "All day" : (start..<max(start, end)).formatted(.interval.weekday().day().month().hour().minute()))
                    .lagomFont(.small).foregroundStyle(Theme.secondary)
                if let location = event.location, !location.isEmpty { Text(location).lagomFont(.base) }
                Text(event.calendar).lagomFont(.small).foregroundStyle(Theme.secondary)
                if let link = event.joinUrl.flatMap(URL.init(string:)), ["https", "http"].contains(link.scheme?.lowercased() ?? "") {
                    Link(destination: link) { ButtonLabel(title: "Join meeting", icon: "video") }.buttonStyle(HouseButtonStyle())
                }
            }
            .padding(16).frame(minWidth: 280, alignment: .leading)
            .presentationCompactAdaptation(.popover)
        }
    }
}

/// The now line across the week, desktop `current-time`: the time in the gutter, a dot on today's column.
private struct NowLine24: View {
    let days: [Date]
    let hour: CGFloat
    let gutter: CGFloat

    var body: some View {
        TimelineView(.everyMinute) { context in
            if let today = days.firstIndex(where: { Calendar.current.isDate($0, inSameDayAs: context.date) }) {
                let c = Calendar.current.dateComponents([.hour, .minute], from: context.date)
                let y = (CGFloat(c.hour ?? 0) + CGFloat(c.minute ?? 0) / 60) * hour
                GeometryReader { geo in
                    let column = (geo.size.width - gutter) / CGFloat(days.count)
                    Rectangle().fill(Theme.now).frame(height: 1.5).padding(.leading, gutter).offset(y: y)
                    Circle().fill(Theme.now).frame(width: 7, height: 7).offset(x: gutter + CGFloat(today) * column - 3, y: y - 2.75)
                    Text(context.date, format: .dateTime.hour().minute()).lagomFont(.small).monospacedDigit().foregroundStyle(Theme.accentInk)
                        .frame(width: gutter - 4).background(Theme.now, in: RoundedRectangle(cornerRadius: 4)).offset(y: y - 9)
                }
                .allowsHitTesting(false)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("Now, \(context.date.formatted(.dateTime.hour().minute()))")
            }
        }
    }
}
