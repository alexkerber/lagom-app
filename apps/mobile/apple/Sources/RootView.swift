import SwiftUI

/// Today, Mail, Calendar and Todo: the lagom bar on a phone, the desktop's sidebar beside a panel on iPad.
struct RootView: View {
    @Bindable var model: MailModel
    let planner: PlannerModel
    // The desktop restores the last page too; Mail on first launch.
    @AppStorage(AppSection.storageKey) private var section: AppSection = .mail
    @Environment(\.horizontalSizeClass) private var width
    @Environment(\.verticalSizeClass) private var height
    @State private var addAccount = false
    @State private var removing: MailAccount?
    /// The phone's Settings sheet; a mailto link or a notification closes it, and Add account inside it.
    @State private var settingsOpen = Storage.settingsScreen != nil
    /// A message Today asked to open once the Inbox scope has reloaded.
    @State private var pending: Int64?

    init(model: MailModel, planner: PlannerModel) {
        self.model = model
        self.planner = planner
        // The unread count stays quiet, as on desktop: grey, not the system red.
        let look = UITabBarAppearance()
        look.configureWithDefaultBackground()
        for item in [look.stackedLayoutAppearance, look.inlineLayoutAppearance, look.compactInlineLayoutAppearance] {
            item.normal.badgeBackgroundColor = UIColor(Theme.secondary.opacity(0.25))
            item.normal.badgeTextAttributes = [.foregroundColor: UIColor(Theme.ink)]
        }
        UITabBar.appearance().standardAppearance = look
    }

    var body: some View {
        Group {
            // The sidebar is for iPad only. An unfolded iPhone Duo is regular both ways too, but keeps the
            // system bar, which iOS puts on its right edge (Alex 2026-11-17).
            if width == .regular && height == .regular && UIDevice.current.userInterfaceIdiom == .pad {
                HStack(spacing: 0) {
                    SidebarView(model: model, planner: planner, section: $section, addAccount: $addAccount, removing: $removing)
                        .frame(width: 240)
                    // The page meets the sidebar at a straight edge and runs to the screen's edges, as desktop.
                    page(section)
                        .background(Theme.background.ignoresSafeArea())
                }
                .background(Theme.sidebar.ignoresSafeArea())
            } else {
                // The system tab bar: native placement, glass and VoiceOver; lagom's icons and a quiet count.
                TabView(selection: $section) {
                    // Settings opens from the mailbox menu on a phone; it is rarely used (Alex 2026-11-17).
                    ForEach(AppSection.allCases.filter { $0 != .settings }) { page in
                        Tab(value: page) {
                            self.page(page)
                                .background(Theme.background.ignoresSafeArea())
                                // A message reads full height; its back button returns to the list and the bar.
                                .toolbar(page == .mail && model.selectedID != nil ? .hidden : .visible, for: .tabBar)
                        } label: {
                            Label { Text(page.title) } icon: { Image(uiImage: tabIcon(page.icon)) }
                        }
                        .badge(page == .mail ? planner.unreadCount : 0)
                    }
                }
                .tint(Theme.selectionIcon)
                .onAppear { if section == .settings { section = .mail } }
                // The Duo's column: lagom's actions join the system bar there.
                .duoRail()
            }
        }
        .foregroundStyle(Theme.ink)
        .sheet(isPresented: $addAccount) { AddAccountView(model: model).duoRail() }
        .modifier(RemoveAccountDialog(model: model, removing: $removing))
        // One reload per scope, even when the mailbox and the account change together.
        .task(id: "\(model.mailbox.rawValue)/\(model.folder?.id ?? 0)/\(model.accountID ?? "")/\(model.searchText)/\(model.core == nil)/\(model.notificationRevision)") {
            await model.changeScope()
            guard !Task.isCancelled else { return }
            let id = pending
            pending = nil
            // A newer scope change cancels this task; never open the message in another scope.
            if let id, model.mailbox == .inbox, model.folder == nil, model.accountID == nil, model.searchText.isEmpty { model.selectedID = id }
        }
        // Opens only while lagom is the default mail app, which needs Apple's mail-client entitlement.
        .onOpenURL { url in
            guard url.scheme?.lowercased() == "mailto" else { return }
            addAccount = false
            settingsOpen = false
            section = .mail
            Task { await model.openMailto(url.absoluteString) }
        }
        .onChange(of: model.notificationRevision, initial: true) { _, _ in
            if model.notificationTarget != nil {
                addAccount = false
                settingsOpen = false
                pending = nil
                section = .mail
            }
        }
        // Unread counts, Today's mail and the sidebar counts follow every mail reload.
        .onChange(of: model.rows) { _, _ in Task { await planner.reloadMail() } }
        // A sync can change other mailboxes, and fetches the calendar, while the visible rows stay the same.
        .onChange(of: model.syncing) { _, syncing in if !syncing { Task { await planner.reloadMail(); await planner.reloadAgenda() } } }
        .onChange(of: section) { _, section in
            if section == .calendar { Task { await planner.reloadAgenda() } }
            if section == .today { Task { await planner.reloadMail() } }
        }
    }

    /// Shows the unified Inbox, and a message in it when given.
    private func openMail(_ id: Int64?) {
        if model.mailbox == .inbox && model.folder == nil && model.accountID == nil && model.searchText.isEmpty {
            if let id { model.selectedID = id }
        } else {
            pending = id
            model.mailbox = .inbox
            model.accountID = nil
            model.searchText = ""
        }
        section = .mail
    }

    @ViewBuilder private func page(_ section: AppSection) -> some View {
        switch section {
        case .today: TodayView(planner: planner, model: model, openMail: openMail, open: { self.section = $0 })
        case .mail: MailView(model: model, planner: planner, settingsOpen: $settingsOpen)
        case .calendar: CalendarView(planner: planner, model: model)
        case .todo: TodoView(planner: planner, openMail: openMail)
        case .settings: SettingsView(model: model)
        }
    }
}

/// The desktop sidebar without the brand (Alex 2026-11-17): Search, the pages, Mail's mailboxes with per-account Inbox, then Accounts.
struct SidebarView: View {
    @Bindable var model: MailModel
    let planner: PlannerModel
    @Binding var section: AppSection
    @Binding var addAccount: Bool
    @Binding var removing: MailAccount?
    @State private var newFolder: NewFolderTarget?

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            SearchField(text: $model.searchText)
                .onChange(of: model.searchText) { _, text in if !text.isEmpty { section = .mail } }
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(AppSection.allCases) { page in
                        SidebarRow(page.title, icon: page.icon, count: page == .mail ? planner.unreadCount : 0, badge: true,
                                   active: section == page, page: true) { section = page }
                    }
                    if section == .mail { mailboxes.padding(.top, 18) }
                    if section == .mail && !model.accounts.isEmpty { folders.padding(.top, 18) }
                    accounts.padding(.top, 18)
                }
            }
            .scrollIndicators(.hidden)
        }
        .padding(.horizontal, 12).padding(.top, 8)
        .foregroundStyle(Theme.ink)
        .modifier(NewFolderAlert(target: $newFolder, model: model))
    }

    private var mailboxes: some View {
        VStack(alignment: .leading, spacing: 2) {
        mailbox(.inbox, count: planner.inboxLeft(account: nil))
        if model.accounts.count > 1 {
            ForEach(Array(model.accounts.enumerated()), id: \.element.id) { index, account in
                SidebarRow(title: account.displayName, count: Int(planner.inboxLeft(account: account.id)),
                           active: model.mailbox == .inbox && model.folder == nil && model.accountID == account.id) {
                    Circle().fill(Theme.account(index)).frame(width: 7, height: 7).frame(width: 18)
                } action: {
                    open(.inbox, account: account.id)
                }
                .padding(.leading, 16)
                .accessibilityLabel("\(account.displayName) Inbox")
            }
        }
        mailbox(.later, count: planner.later(account: nil))
        ForEach(MailboxChoice.allCases.filter { $0 != .inbox && $0 != .later }) { mailbox($0, count: 0) }
        }
    }

    /// Desktop's folder group: the server's own folders, by account when there are several.
    private var folders: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Folders").lagomFont(.small, .medium).foregroundStyle(Theme.secondary)
                .padding(.horizontal, 10).padding(.bottom, 4).accessibilityAddTraits(.isHeader)
            ForEach(model.accounts, id: \.id) { account in
                let own = model.folders.filter { $0.account == account.id }
                if model.accounts.count > 1 {
                    Text(account.displayName).lagomFont(.small).foregroundStyle(Theme.secondary).lineLimit(1)
                        .padding(.horizontal, 10).padding(.top, 6)
                }
                ForEach(own, id: \.id) { folder in
                    SidebarRow(folder.name, icon: "folder", count: Int(folder.unread), active: section == .mail && model.folder?.id == folder.id) {
                        model.open(folder)
                        section = .mail
                    }
                    .padding(.leading, CGFloat(folder.depth) * 16)
                    .accessibilityLabel(folder.path)
                }
                // Desktop's "New folder" row, in each account's group.
                SidebarRow("New folder", icon: "new-folder") { newFolder = NewFolderTarget(account: account.id) }.disabled(model.working)
                    .accessibilityLabel(model.accounts.count > 1 ? "New folder in \(account.displayName)" : "New folder")
            }
        }
    }

    private var accounts: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Accounts").lagomFont(.small, .medium).foregroundStyle(Theme.secondary)
                .padding(.horizontal, 10).padding(.bottom, 4).accessibilityAddTraits(.isHeader)
            ForEach(Array(model.accounts.enumerated()), id: \.element.id) { index, account in
                HStack(spacing: 10) {
                    Circle().fill(model.accounts.count > 1 ? Theme.account(index) : Theme.accent).frame(width: 8, height: 8)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(account.displayName).lagomFont(.base, .medium).lineLimit(1)
                        if account.displayName != account.email {
                            Text(account.email).lagomFont(.small).foregroundStyle(Theme.secondary).lineLimit(1)
                        }
                    }
                }
                .padding(.horizontal, 10).frame(minHeight: 46)
                .accessibilityElement(children: .combine)
                .contextMenu { Button("Remove account", role: .destructive) { removing = account } }
                .accessibilityAction(named: "Remove account") { removing = account }
            }
            SidebarRow("Add account", icon: "account-add") { addAccount = true }
        }
    }

    private func mailbox(_ box: MailboxChoice, count: UInt64) -> some View {
        SidebarRow(box.rawValue, icon: box.icon, count: Int(count),
                   active: section == .mail && model.folder == nil && model.mailbox == box && (box != .inbox || model.accountID == nil)) {
            open(box, account: nil)
        }
    }

    private func open(_ box: MailboxChoice, account: String?) {
        model.accountID = account
        model.mailbox = box
        section = .mail
    }
}
