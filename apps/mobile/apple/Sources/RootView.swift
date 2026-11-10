import SwiftUI

/// Today, Mail, Calendar and Todo: tabs on a phone, the desktop's sidebar on iPad and unfolded screens.
struct RootView: View {
    @Bindable var model: MailModel
    let planner: PlannerModel
    // The desktop restores the last page too; Mail on first launch.
    @AppStorage(AppSection.storageKey) private var section: AppSection = .mail
    @Environment(\.horizontalSizeClass) private var width
    @Environment(\.verticalSizeClass) private var height
    @State private var addAccount = false
    @State private var removing: MailAccount?
    /// A message Today asked to open once the Inbox scope has reloaded.
    @State private var pending: Int64?

    var body: some View {
        Group {
            if width == .regular && height == .regular {
                if section == .mail {
                    mail
                } else {
                    NavigationSplitView {
                        sidebar
                    } detail: {
                        NavigationStack { page(section) }
                    }
                }
            } else {
                TabView(selection: $section) {
                    Tab(AppSection.today.title, systemImage: AppSection.today.symbol, value: .today) {
                        NavigationStack { page(.today) }
                    }
                    Tab(AppSection.mail.title, systemImage: AppSection.mail.symbol, value: .mail) { mail }
                        .badge(planner.unreadCount)
                    Tab(AppSection.calendar.title, systemImage: AppSection.calendar.symbol, value: .calendar) {
                        NavigationStack { page(.calendar) }
                    }
                    Tab(AppSection.todo.title, systemImage: AppSection.todo.symbol, value: .todo) {
                        NavigationStack { page(.todo) }
                    }
                }
            }
        }
        .sheet(isPresented: $addAccount) { AddAccountView(model: model) }
        .alert("lagom", isPresented: Binding(get: { model.error != nil || planner.error != nil },
                                            set: { if !$0 { model.error = nil; planner.error = nil } })) {
            Button("OK") { model.error = nil; planner.error = nil }
        } message: { Text(model.error ?? planner.error ?? "") }
        .confirmationDialog("Remove account and its local mail?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } })) {
            if let account = removing {
                Button("Remove \(account.email)", role: .destructive) { Task { await model.remove(account) }; removing = nil }
            }
        }
        // One reload per scope, even when the mailbox and the account change together.
        .task(id: "\(model.mailbox.rawValue)/\(model.accountID ?? "")/\(model.searchText)") {
            await model.changeScope()
            guard !Task.isCancelled else { return }
            let id = pending
            pending = nil
            // A newer scope change cancels this task; never open the message in another scope.
            if let id, model.mailbox == .inbox, model.accountID == nil, model.searchText.isEmpty { model.selectedID = id }
        }
        // Unread counts, Today's mail and the sidebar counts follow every mail reload.
        .onChange(of: model.rows) { _, _ in Task { await planner.reloadMail() } }
        // A sync can change other mailboxes while the visible rows stay the same.
        .onChange(of: model.syncing) { _, syncing in if !syncing { Task { await planner.reloadMail() } } }
        .onChange(of: section) { _, section in
            if section == .calendar { Task { await planner.reloadAgenda() } }
            if section == .today { Task { await planner.reloadMail() } }
        }
    }

    private var mail: some View {
        MailView(model: model, planner: planner, section: $section, addAccount: $addAccount, removing: $removing)
    }

    /// Shows the unified Inbox, and a message in it when given.
    private func openMail(_ id: Int64?) {
        if model.mailbox == .inbox && model.accountID == nil && model.searchText.isEmpty {
            if let id { model.selectedID = id }
        } else {
            pending = id
            model.mailbox = .inbox
            model.accountID = nil
            model.searchText = ""
        }
        section = .mail
    }

    private var sidebar: some View {
        SidebarView(model: model, planner: planner, section: $section, addAccount: $addAccount, removing: $removing)
    }

    @ViewBuilder private func page(_ section: AppSection) -> some View {
        switch section {
        case .today: TodayView(planner: planner, model: model, openMail: openMail)
        case .mail: mail
        case .calendar: CalendarView(planner: planner, model: model)
        case .todo: TodoView(planner: planner)
        }
    }
}

enum SidebarItem: Hashable {
    case section(AppSection)
    case mailbox(MailboxChoice, account: String?)
}

/// The desktop sidebar: the four pages, Mail's mailboxes with per-account Inbox, then Accounts.
struct SidebarView: View {
    @Bindable var model: MailModel
    let planner: PlannerModel
    @Binding var section: AppSection
    @Binding var addAccount: Bool
    @Binding var removing: MailAccount?

    var body: some View {
        List(selection: selection) {
            Section {
                ForEach(AppSection.allCases) { page in
                    label(page.title, page.symbol, .section(page))
                        .badge(page == .mail ? planner.unreadCount : 0)
                        .tag(SidebarItem.section(page))
                }
            }
            if section == .mail {
                Section("Mailboxes") {
                    mailbox(.inbox, count: planner.inboxLeft(account: nil))
                    if model.accounts.count > 1 {
                        ForEach(Array(model.accounts.enumerated()), id: \.element.id) { index, account in
                            Label { Text(account.displayName).lineLimit(1) } icon: {
                                Circle().fill(Theme.account(index)).frame(width: 8, height: 8)
                            }
                            .padding(.leading, 16)
                            .badge(Int(planner.inboxLeft(account: account.id)))
                            .tag(SidebarItem.mailbox(.inbox, account: account.id))
                            .accessibilityLabel("\(account.displayName) Inbox")
                        }
                    }
                    mailbox(.later, count: planner.later(account: nil))
                    ForEach([MailboxChoice.sent, .done, .trash]) { mailbox($0, count: 0) }
                }
            }
            Section("Accounts") {
                ForEach(Array(model.accounts.enumerated()), id: \.element.id) { index, account in
                    HStack(spacing: 10) {
                        Circle().fill(model.accounts.count > 1 ? Theme.account(index) : Theme.secondary).frame(width: 8, height: 8)
                            .accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(account.displayName).lineLimit(1)
                            if account.displayName != account.email {
                                Text(account.email).font(.footnote).foregroundStyle(Theme.secondary).lineLimit(1)
                            }
                        }
                    }
                    .accessibilityElement(children: .combine)
                    .contextMenu { Button("Remove account", systemImage: "trash", role: .destructive) { removing = account } }
                }
                Button { addAccount = true } label: { Label("Add account", systemImage: "plus") }
                    .foregroundStyle(Theme.accent)
            }
        }
        // The desktop's selection fill: the light accent in both modes, so white text keeps 4.5:1.
        .tint(Theme.selection)
        .navigationTitle("lagom")
    }

    private func mailbox(_ box: MailboxChoice, count: UInt64) -> some View {
        label(box.rawValue, box.symbol, .mailbox(box, account: nil))
            .badge(Int(count))
            .tag(SidebarItem.mailbox(box, account: nil))
    }

    /// Grey icons as on desktop; white on the selection fill.
    private func label(_ title: String, _ symbol: String, _ item: SidebarItem) -> some View {
        Label { Text(title) } icon: {
            Image(systemName: symbol).foregroundStyle(selection.wrappedValue == item ? Color.white : Theme.secondary)
        }
    }

    private var selection: Binding<SidebarItem?> {
        Binding {
            section == .mail ? .mailbox(model.mailbox, account: model.mailbox == .inbox ? model.accountID : nil) : .section(section)
        } set: { item in
            switch item {
            case .section(let page): section = page
            case .mailbox(let box, let account):
                model.accountID = account
                model.mailbox = box
                section = .mail
            case nil: break
            }
        }
    }
}
