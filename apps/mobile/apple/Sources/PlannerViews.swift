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
    let open: (AppSection) -> Void
    @Environment(\.horizontalSizeClass) private var width
    /// Dynamic Type scale of the display size, so tracking stays the same share of an em.
    @ScaledMetric(relativeTo: .largeTitle) private var typeScale: CGFloat = 1
    @State private var newTask = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 32) {
                sentence
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
        .background(Theme.background)
        .sheet(isPresented: $newTask) { NewTaskView(planner: planner, list: planner.defaultList, due: Date()).duoRail() }
    }

    /// The desktop's Today sentence in two tones (Alex 2026-11-17): the counts are ink and open their page.
    private var sentence: some View {
        let now = Date.now
        let greeting = switch Calendar.current.component(.hour, from: now) {
        case 0..<12: "Good morning."
        case 12..<18: "Good afternoon."
        default: "Good evening."
        }
        func count(_ n: Int, _ one: String, _ many: String, _ none: String) -> String { n == 0 ? none : n == 1 ? "1 \(one)" : "\(n) \(many)" }
        func part(_ text: String, _ link: String? = nil) -> AttributedString {
            var part = AttributedString(text)
            // A link run keeps its own colour: ink for the counts, the words between them ink2.
            part.foregroundColor = link == nil ? Theme.secondary : Theme.ink
            if let link { part.link = URL(string: "lagom-today:\(link)") }
            return part
        }
        var line = AttributedString(greeting + "\n")
        line.foregroundColor = Theme.ink
        line += part(count(planner.unreadCount, "unread", "unread", "Nothing unread"), "mail") + part(", ")
        line += part(count(planner.today?.events.count ?? 0, "event", "events", "no events"), "calendar") + part(" and ")
        line += part(count(dueCount(planner.today?.tasks ?? [], day: planner.today?.day ?? ""), "task due", "tasks due", "nothing due"), "todo") + part(".")
        return VStack(alignment: .leading, spacing: 12) {
            MonoLabel(text: now.formatted(.dateTime.weekday(.wide).day().month(.wide)))
            Text(line)
                .font(.custom(Theme.Face.display, size: width == .regular ? 48 : 34, relativeTo: .largeTitle))
                .tracking((width == .regular ? 48 : 34) * typeScale * Theme.Face.displayTracking)
                .lineSpacing(-2).tint(Theme.ink).accessibilityAddTraits(.isHeader)
                .environment(\.openURL, OpenURLAction { url in
                    switch url.absoluteString {
                    case "lagom-today:mail": openMail(nil)
                    case "lagom-today:calendar": open(.calendar)
                    case "lagom-today:todo": open(.todo)
                    default: return .systemAction
                    }
                    return .handled
                })
        }
    }

    private var schedule: some View {
        let events = planner.today?.events ?? []
        let now = Date()
        // The now line sits before the first timed event still to start, as on desktop.
        let nowIndex = events.firstIndex { !$0.allDay && Date(timeIntervalSince1970: TimeInterval($0.startUnix)) > now } ?? events.count
        return VStack(alignment: .leading, spacing: 4) {
            MonoLabel(text: "Schedule").padding(.bottom, 8)
            if events.isEmpty {
                quiet(planner.calendarConnected ? "Nothing scheduled today."
                      : !planner.calendarAccess.isEmpty ? "Google Calendar is not connected."
                      : planner.calendarGranted ? "Your calendar shows here after the next sync."
                      : "Add a Google account to see your calendar here.", icon: "calendar")
            } else {
                ForEach(Array(events.enumerated()), id: \.offset) { index, event in
                    if index == nowIndex { NowLine(now: now) }
                    EventRow(event: event, accent: accountAccent(model.accounts, event.account), now: now)
                }
                if nowIndex == events.count { NowLine(now: now) }
            }
            // Also beside another account's calendar: each Google account allows its own.
            if let account = planner.calendarAccess.first {
                CalendarAccessButton(planner: planner, model: model, account: account).padding(.top, 4)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var tasks: some View {
        VStack(alignment: .leading, spacing: 0) {
            MonoLabel(text: "To-do").padding(.bottom, 8)
            let tasks = planner.today?.tasks ?? []
            if tasks.isEmpty { quiet("Nothing due today.", icon: "todo").padding(.bottom, 6) }
            ForEach(tasks, id: \.id) { todo in
                TodoRow(todo: todo, toggle: { await planner.toggle(todo, settle: tickSettle) },
                        openMail: { Task { if let id = await planner.mail(for: todo) { openMail(id) } } })
            }
            AddRow(title: "New task") { newTask = true }.disabled(planner.lists.isEmpty)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var mail: some View {
        VStack(alignment: .leading, spacing: 4) {
            MonoLabel(text: "Mail").padding(.bottom, 8)
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

/// Small Geist Mono capitals, the site's labels: Today's date and section titles.
struct MonoLabel: View {
    let text: String
    var body: some View {
        Text(text.uppercased()).font(.custom(Theme.Face.label, size: 13, relativeTo: .footnote))
            .foregroundStyle(Theme.secondary).accessibilityLabel(text).accessibilityAddTraits(.isHeader)
    }
}

private func quiet(_ text: String, icon: String) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: 8) {
        Icon(icon).alignmentGuide(.firstTextBaseline) { $0[.bottom] - 3 }
        Text(text).lagomFont(.snippet)
    }
    .foregroundStyle(Theme.secondary)
}

/// Desktop's "Show your Google Calendar": Google asks once, then a sync fills Calendar and Today.
private struct CalendarAccessButton: View {
    let planner: PlannerModel
    let model: MailModel
    let account: MailAccount
    @Environment(\.webAuthenticationSession) private var browser
    @State private var connecting = false

    var body: some View {
        Button { Task { await allow() } } label: {
            HStack(spacing: 6) {
                if connecting { ProgressView().controlSize(.small).tint(Theme.accentInk) } else { Icon("calendar").foregroundStyle(Theme.accentInk) }
                Text(connecting ? "Connecting…" : "Show your Google Calendar").lineLimit(1)
            }
        }
        .buttonStyle(HouseButtonStyle(primary: true))
        .disabled(connecting)
    }

    private func allow() async {
        defer { connecting = false }
        do {
            // The button stays busy until the sync has fetched the calendar.
            guard try await model.allowCalendar(account.id, browser: browser, exchanging: { connecting = true }) else { return }
            await model.sync()
            await planner.reloadAgenda()
        } catch { planner.error = error.localizedDescription }
    }
}

/// "+ New task", "+ Add task": a quiet line that adds.
private struct AddRow: View {
    let title: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                // The task circles' column, so the plus sits over them and the words over the titles.
                Icon("plus").frame(width: 21)
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

/// How long a tapped tick shows before its task moves to Completed: within motion.md's 300 ms.
private let tickSettle = Duration.milliseconds(280)

/// A task, desktop `task_row`: a completion circle, its title over the source mail's subject, and the due chip.
struct TodoRow: View {
    let todo: Todo
    /// Saves the change; returns once the lists have reloaded.
    let toggle: () async -> Void
    /// Opens the source mail; the subject line is its button.
    let openMail: () -> Void
    var words = DateWords()
    /// The tick a tap draws at once, before the row moves (`PlannerModel.toggle`'s settle).
    @State private var ticked: Bool?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let done = ticked ?? todo.done
        HStack(spacing: 10) {
            Button(action: tap) {
                CheckCircle(done: done).frame(width: 44, height: 44).contentShape(Rectangle())
            }
            .buttonStyle(.plain).padding(.horizontal, -11)
            .accessibilityLabel(done ? "Mark as not done" : "Mark as done")
            VStack(alignment: .leading, spacing: 2) {
                Text(todo.title).lagomFont(.base)
                    .strikethrough(done).foregroundStyle(done ? Theme.secondary : Theme.ink)
                if let subject = todo.mailSubject {
                    Button(action: openMail) {
                        HStack(spacing: 4) {
                            Icon("mail", size: 14)
                            Text(subject).lagomFont(.small).lineLimit(1).truncationMode(.tail)
                        }
                        // A 44 pt tall target without a taller row, as the completion circle.
                        .foregroundStyle(Theme.secondary).padding(.vertical, 15).contentShape(Rectangle())
                    }
                    .buttonStyle(.plain).padding(.vertical, -15)
                    .accessibilityLabel("Open mail: \(subject)")
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if todo.fromMail, todo.mailSubject == nil {
                Icon("mail", label: "From mail").foregroundStyle(Theme.secondary)
            }
            if !done, let due = todo.due.flatMap(words.due) { DueChip(label: due.label, overdue: due.overdue) }
        }
        .padding(.horizontal, 8)
        .frame(minHeight: 46)
        .accessibilityElement(children: .contain)
        .sensoryFeedback(.impact(weight: .light), trigger: ticked) { _, new in new == true }
    }

    private func tap() {
        guard ticked == nil else { return } // one change at a time
        withAnimation(Motion.move(reduceMotion)) { ticked = !todo.done }
        // Saved at once; the reload after the settle clears the tick, or brings the circle back if core refused.
        Task { await toggle(); ticked = nil }
    }
}

/// The completion circle: a hairline ring that fills with Done's pine green and a tick, springing in.
private struct CheckCircle: View {
    let done: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ZStack {
            Circle().strokeBorder(Theme.secondary, lineWidth: 1).opacity(done ? 0 : 1)
            Circle().fill(Theme.done)
                .overlay { Icon("done", size: 15).foregroundStyle(Theme.accentInk) }
                // Under Reduce Motion it only fades.
                .scaleEffect(done || reduceMotion ? 1 : 0.5)
                .opacity(done ? 1 : 0)
        }
        .frame(width: 21, height: 21)
    }
}

/// A due date, desktop's chip: quiet, or red once overdue.
struct DueChip: View {
    let label: String
    var overdue = false

    var body: some View {
        Text(label).lagomFont(.small)
            .foregroundStyle(overdue ? Theme.danger : Theme.secondary)
            .padding(.horizontal, 7).padding(.vertical, 3)
            .background(overdue ? Theme.danger.opacity(0.12) : Theme.subtle, in: RoundedRectangle(cornerRadius: Theme.Radius.key))
            .accessibilityLabel(overdue ? "Overdue, due \(label)" : "Due \(label)")
    }
}

/// What the inline field adds: a task to a list, or a new list.
private enum AddField: Hashable { case task(Int64), list }

/// Local task lists, as on the desktop's To-do page. Adding happens where the eye is (Alex 2026-11-20, "plus
/// uppe i hörnet … old school"): "New task" ends each list and turns into a field in place, with due chips
/// above the keyboard. Return adds and keeps the field open for the next task; leaving the field keeps its text.
struct TodoView: View {
    let planner: PlannerModel
    let openMail: (Int64?) -> Void
    @State private var showDone: Set<Int64> = []
    /// Rows a swipe just completed or deleted: gone at once, back only if core refuses.
    @State private var leaving: Set<Int64> = []
    @State private var adding: AddField?
    @State private var title = ""
    @State private var due: Date?
    @State private var picking = false
    @FocusState private var focus: AddField?
    @Environment(\.horizontalSizeClass) private var width
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @ScaledMetric(relativeTo: .largeTitle) private var typeScale: CGFloat = 1

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            PageTitle(title: "To-do") { EmptyView() }
                .modifier(PagePadding()).padding(.top, 8)
            List {
                if planner.allClear { clearDay.plainRow() }
                ForEach(Array(planner.lists.enumerated()), id: \.element.id) { index, list in
                    section(list, index: index)
                }
                newList.padding(.top, 16).plainRow()
            }
            .listStyle(.plain)
            .environment(\.defaultMinListRowHeight, 0)
            .scrollContentBackground(.hidden)
            .scrollDismissesKeyboard(.interactively)
            .modifier(PagePadding())
            // Rows close the gap as they leave for Completed; under Reduce Motion they fade.
            .animation(Motion.move(reduceMotion), value: planner.todos)
            .overlay {
                if planner.lists.isEmpty && adding != .list {
                    EmptyState(icon: "todo", title: "No lists yet", detail: "A list holds what there is to do.") {
                        HouseButton(title: "New list", icon: "plus") { open(.list) }
                    }
                }
            }
            .refreshable { await planner.reloadTodos() }
        }
        .background(Theme.background)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if case .task = adding { dueBar }
        }
        .onChange(of: focus) { _, new in
            // Leaving the field, by a tap elsewhere or the keyboard going down, keeps what was typed. Checked a turn
            // later: moving to another list's field passes through nil.
            guard new == nil else { return }
            Task { @MainActor in if focus == nil, !picking, adding != nil { commit() } }
        }
        .onChange(of: picking) { _, now in if !now, let adding { focus = adding } }
    }

    private func section(_ list: TodoList, index: Int) -> some View {
        let all = (planner.todos[list.id] ?? []).filter { !leaving.contains($0.id) }
        let done = all.filter(\.done)
        let expanded = showDone.contains(list.id)
        return Group {
            ListHeading(name: list.name, color: Theme.account(index), open: all.count - done.count)
                .padding(.top, index == 0 && !planner.allClear ? 8 : 24).padding(.bottom, 2)
            ForEach(all.filter { !$0.done }, id: \.id) { row($0) }
            if adding == .task(list.id) { field(.task(list.id)) } else {
                AddRow(title: "New task") { open(.task(list.id)) }
            }
            if !done.isEmpty {
                Button {
                    if expanded { showDone.remove(list.id) } else { showDone.insert(list.id) }
                } label: {
                    HStack(spacing: 10) {
                        Icon(expanded ? "chevron" : "chevron-right", size: 16).frame(width: 21)
                        Text("Completed (\(done.count))").lagomFont(.base)
                    }
                    .foregroundStyle(Theme.secondary).padding(.horizontal, 8)
                    .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityValue(expanded ? "Expanded" : "Collapsed")
                if expanded { ForEach(done, id: \.id) { row($0) } }
            }
        }
        .plainRow()
    }

    /// The clear day in lagom's voice, as the desktop: the lists and their New task lines stay below.
    private var clearDay: some View {
        let size: CGFloat = width == .regular ? 48 : 34
        return VStack(alignment: .leading, spacing: 10) {
            Text("Nothing to do.\nA clear day.")
                .font(.custom(Theme.Face.display, size: size, relativeTo: .largeTitle))
                .tracking(size * typeScale * Theme.Face.displayTracking).lineSpacing(-2)
                .foregroundStyle(Theme.ink).accessibilityAddTraits(.isHeader)
            Text("Add a task below when something comes up.").lagomFont(.snippet).foregroundStyle(Theme.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, 24).padding(.bottom, 8)
    }

    @ViewBuilder private var newList: some View {
        if adding == .list { field(.list) } else if !planner.lists.isEmpty {
            AddRow(title: "New list") { open(.list) }
        }
    }

    /// "New task" (or "New list") as a field in place: the circle (or the list dot) where the row will be.
    private func field(_ target: AddField) -> some View {
        HStack(spacing: 10) {
            Group {
                if target == .list { Circle().fill(Theme.line).frame(width: 8, height: 8) }
                else { Circle().strokeBorder(Theme.line, lineWidth: 1).frame(width: 21, height: 21) }
            }
            .frame(width: 21)
            TextField(target == .list ? "List name" : "New task", text: $title)
                .lagomFont(.base, target == .list ? .bold : .regular)
                .foregroundStyle(Theme.ink).tint(Theme.accent)
                .focused($focus, equals: target)
                .submitLabel(target == .list ? .done : .next)
                .onSubmit(submit)
                .onAppear { focus = target }
                .accessibilityLabel(target == .list ? "New list name" : "New task")
            if target != .list, let due, let chip = DateWords().due(DateWords().coreDay(due)) {
                DueChip(label: chip.label)
            }
        }
        .padding(.horizontal, 8)
        .frame(minHeight: 46)
    }

    /// Due chips above the keyboard while a task is typed: Today, Tomorrow, or a date.
    private var dueBar: some View {
        let calendar = Calendar.current
        let today = calendar.startOfDay(for: Date())
        let tomorrow = calendar.date(byAdding: .day, value: 1, to: today) ?? today
        let other = due.map { $0 != today && $0 != tomorrow } ?? false
        return HStack(spacing: 8) {
            chip("Today", on: due == today) { due = due == today ? nil : today }
            chip("Tomorrow", on: due == tomorrow) { due = due == tomorrow ? nil : tomorrow }
            chip(other ? DateWords().due(DateWords().coreDay(due!))?.label ?? "Date" : "Date", icon: "calendar", on: other) { picking = true }
                // A paper sheet, not a glass popover: the calendar needs more room than the folded Duo has above the keyboard.
                .sheet(isPresented: $picking) {
                    DatePicker("Due", selection: Binding(get: { due ?? tomorrow }, set: { due = calendar.startOfDay(for: $0); picking = false }),
                               in: today..., displayedComponents: .date)
                        .datePickerStyle(.graphical).tint(Theme.accent).labelsHidden()
                        .padding(.horizontal, 16)
                        .presentationDetents([.medium])
                        .presentationBackground(Theme.background)
                        .duoRail()
                }
            Spacer(minLength: 0)
            IconButton(title: "Close", icon: "close") { focus = nil }
        }
        .padding(.horizontal, 16).padding(.vertical, 4)
        .background(Theme.background)
        .overlay(alignment: .top) { Theme.line.frame(height: 1) }
    }

    private func chip(_ title: String, icon: String? = nil, on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 6) {
                if let icon { Icon(icon, size: 16) }
                Text(title).lagomFont(.small, .medium).lineLimit(1)
            }
            .foregroundStyle(on ? Theme.accentFillInk : Theme.ink)
            .padding(.horizontal, 10).frame(minHeight: 32)
            .background(on ? Theme.accentFill : Theme.subtle, in: RoundedRectangle(cornerRadius: Theme.Radius.control))
            .frame(minHeight: 44).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Due \(title)")
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    private func open(_ target: AddField) {
        commit()
        adding = target
        focus = target
    }

    /// Return: the task is added and the field stays for the next; an empty Return, or a list name, closes it.
    private func submit() {
        guard case .task(let list)? = adding, !title.trimmingCharacters(in: .whitespaces).isEmpty else {
            commit()
            focus = nil
            return
        }
        let text = title, due = due
        title = ""
        Task { await planner.add(title: text, list: list, due: due) }
        // Return gives up focus; taking it back in the same turn keeps the keyboard up.
        focus = .task(list)
    }

    /// Saves what the field holds and closes it.
    private func commit() {
        let text = title.trimmingCharacters(in: .whitespacesAndNewlines)
        switch adding {
        case .task(let list)? where !text.isEmpty:
            let due = due
            Task { await planner.add(title: text, list: list, due: due) }
        case .list? where !text.isEmpty: Task { await planner.addList(name: text) }
        default: break
        }
        title = ""
        adding = nil
        due = nil
    }

    private func row(_ todo: Todo) -> some View {
        TodoRow(todo: todo, toggle: { await planner.toggle(todo, settle: tickSettle) },
                openMail: { Task { if let id = await planner.mail(for: todo) { openMail(id) } } })
            // Right is Done (Not done in Completed), a long left is Delete, as in Mail; tasks have no Later.
            .modifier(StageSwipe(canDelete: true, enabled: true, canLater: false, doneTitle: todo.done ? "Not done" : "Done") { stage in
                leaving.insert(todo.id)
                Task {
                    if stage == .delete { await planner.delete(todo) } else { await planner.toggle(todo) }
                    leaving.remove(todo.id)
                }
            })
            .accessibilityAction(named: "Delete") { Task { await planner.delete(todo) } }
    }
}

private extension View {
    /// A List row drawn by the page itself: no insets, separator or row background.
    func plainRow() -> some View {
        listRowInsets(EdgeInsets()).listRowSeparator(.hidden).listRowBackground(Color.clear)
    }
}

/// A list's heading on To-do: `● Work  4`, the dot over the task circles and the name over their titles.
// shortcut: the dot colour follows list order; store a colour per list if people ask to choose one.
private struct ListHeading: View {
    let name: String
    let color: Color
    let open: Int

    var body: some View {
        HStack(spacing: 10) {
            Circle().fill(color).frame(width: 8, height: 8).frame(width: 21)
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(name).font(.custom(Theme.Face.display, size: TextSize.base.points, relativeTo: .body)).foregroundStyle(Theme.ink)
                if open > 0 {
                    Text(String(open)).font(.custom(Theme.Face.label, size: TextSize.small.points, relativeTo: .footnote)).foregroundStyle(Theme.secondary)
                }
            }
        }
        .padding(.horizontal, 8)
        .frame(minHeight: 36)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(open > 0 ? "\(name), \(open) open" : name)
        .accessibilityAddTraits(.isHeader)
    }
}

/// Today's New task sheet; To-do adds in place.
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

/// The desktop week on iPad; a phone shows one day of it under a week strip, as Spark on the Duo (Alex 2026-11-17).
struct CalendarView: View {
    let planner: PlannerModel
    let model: MailModel
    @Environment(\.horizontalSizeClass) private var width

    var body: some View {
        // A landscape Pro Max is regular width too; only iPad gets the whole week.
        WeekGrid(planner: planner, model: model, oneDay: !(width == .regular && UIDevice.current.userInterfaceIdiom == .pad))
    }
}

/// The desktop week (`week::week_view`): day columns on an hour grid, all-day events above, the now line across.
private struct WeekGrid: View {
    let planner: PlannerModel
    let model: MailModel
    let oneDay: Bool
    @State private var selected = Calendar.current.startOfDay(for: .now)
    @Environment(\.railInset) private var railInset
    private static let hour: CGFloat = 60
    private static let gutter: CGFloat = 48
    private let calendar = Calendar.current
    @State private var position = ScrollPosition(edge: .top)
    @State private var allDayHeight: CGFloat = 0

    var body: some View {
        let week = (0..<7).compactMap { calendar.date(byAdding: .day, value: $0, to: planner.weekStart) }
        let days = oneDay ? [selected] : week
        let byDay = Dictionary(grouping: planner.week, by: \.day)
        let words = DateWords()
        VStack(alignment: .leading, spacing: 8) {
            header
            if oneDay { strip(week) }
            if !planner.calendarConnected && planner.week.isEmpty, let account = planner.calendarAccess.first {
                EmptyState(icon: "calendar", title: "Calendar is not connected", detail: "Google asks once for access to \(account.email).") {
                    CalendarAccessButton(planner: planner, model: model, account: account)
                }
                .frame(maxHeight: .infinity)
            } else if !planner.calendarConnected && planner.week.isEmpty && planner.calendarGranted {
                EmptyState(icon: "calendar", title: "Calendar not synced yet", detail: "Your calendar shows here after the next sync.")
                    .frame(maxHeight: .infinity)
            } else if !planner.calendarConnected && planner.week.isEmpty {
                EmptyState(icon: "calendar", title: "Calendar needs a Google account",
                           detail: "Add a Google account to see your calendar here. Accounts with a password have no calendar.")
                    .frame(maxHeight: .infinity)
            } else {
                if let account = planner.calendarAccess.first {
                    CalendarAccessButton(planner: planner, model: model, account: account)
                }
                if !oneDay {
                    HStack(spacing: 0) {
                        Color.clear.frame(width: Self.gutter, height: 1)
                        ForEach(days, id: \.self) { WeekDayHead(day: $0).frame(maxWidth: .infinity, alignment: .leading) }
                    }
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
        .padding(.horizontal, oneDay ? 16 : 24).padding(.top, 8)
        .background(Theme.background)
        .task { await planner.reloadWeek() }
        // A new week opens on today when it holds today, else on its Monday.
        .onChange(of: planner.weekStart, initial: true) { _, start in
            selected = calendar.isDate(PlannerModel.monday(.now), inSameDayAs: start) ? calendar.startOfDay(for: .now) : start
        }
        .railColumn(top: {
            if oneDay {
                VStack(spacing: 8) {
                    RailButton(title: "Previous week", icon: "chevron-left") { Task { await planner.showWeek(-1) } }
                    RailButton(title: "Today", icon: "calendar") { selected = calendar.startOfDay(for: .now); Task { await planner.showWeek(0) } }
                    RailButton(title: "Next week", icon: "chevron-right") { Task { await planner.showWeek(1) } }
                }
            }
        }, bottom: { EmptyView() })
    }

    /// The week as seven tappable days, Spark's strip: the chosen day in the accent circle, today in the accent.
    private func strip(_ week: [Date]) -> some View {
        HStack(spacing: 0) {
            ForEach(week, id: \.self) { day in
                let chosen = calendar.isDate(day, inSameDayAs: selected)
                let today = calendar.isDateInToday(day)
                Button { selected = day } label: {
                    VStack(spacing: 4) {
                        Text(day, format: .dateTime.weekday(.narrow)).lagomFont(.small).foregroundStyle(Theme.secondary)
                        Text(day, format: .dateTime.day()).lagomFont(.title).monospacedDigit()
                            .foregroundStyle(chosen ? Theme.accentInk : today ? Theme.accent : Theme.ink)
                            .frame(width: 40, height: 40)
                            .background(chosen ? Theme.accent : .clear, in: Circle())
                    }
                    .frame(maxWidth: .infinity, minHeight: 44).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(day.formatted(.dateTime.weekday(.wide).day().month(.wide)))
                .accessibilityAddTraits(chosen ? [.isSelected, .isButton] : .isButton)
            }
        }
    }

    private var header: some View {
        // The month a week belongs to is its Thursday's, as ISO weeks count.
        let thursday = calendar.date(byAdding: .day, value: 3, to: planner.weekStart) ?? planner.weekStart
        let month = oneDay ? selected : thursday
        return HStack(spacing: 8) {
            HStack(spacing: 6) {
                Text(month, format: .dateTime.month(.wide)).font(.custom(Theme.Face.display, size: TextSize.subject.points, relativeTo: .title2))
                Text(month, format: .dateTime.year()).lagomFont(.subject).foregroundStyle(Theme.secondary)
            }
            .accessibilityElement(children: .combine).accessibilityAddTraits(.isHeader)
            Spacer(minLength: 8)
            if !(oneDay && railInset > 0) {
                IconButton(title: "Previous week", icon: "chevron-left") { Task { await planner.showWeek(-1) } }
                Button("Today") { selected = calendar.startOfDay(for: .now); Task { await planner.showWeek(0) } }.buttonStyle(HouseButtonStyle())
                IconButton(title: "Next week", icon: "chevron-right") { Task { await planner.showWeek(1) } }
            }
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
        .task(id: "\(planner.weekStart) \(first) \(oneDay ? selected : planner.weekStart)") { position.scrollTo(y: CGFloat(first) * Self.hour) }
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

/// ForEach keys from everything the grid shows of an event, not where it sits. Copies that differ in
/// nothing shown are numbered to stay unique; if they swap numbers, the popover still shows the same content.
private func eventKeys(_ events: [AgendaItem]) -> [[String?]] {
    var seen: [[String?]: Int] = [:]
    return events.map { e in
        let key: [String?] = [e.account, e.calendar, "\(e.startUnix)", "\(e.endUnix)", e.title, e.location, e.joinUrl]
        seen[key, default: 0] += 1
        return key + ["\(seen[key]!)"]
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
                        .lineLimit(1).minimumScaleFactor(0.6) // "3:53 PM" must fit the gutter on one line
                        .frame(width: gutter - 4).background(Theme.now, in: RoundedRectangle(cornerRadius: 4)).offset(y: y - 9)
                }
                .allowsHitTesting(false)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("Now, \(context.date.formatted(.dateTime.hour().minute()))")
            }
        }
    }
}
