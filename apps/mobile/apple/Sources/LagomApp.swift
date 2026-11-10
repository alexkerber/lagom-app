import SwiftUI

@main
struct LagomApp: App {
    @State private var model = MailModel()
    @State private var planner = PlannerModel()
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            RootView(model: model, planner: planner)
                .tint(Theme.accent)
                .task { await model.start() }
                // Local tasks and events load as soon as the store opens, not after the first sync.
                .task(id: model.core == nil) {
                    guard let core = model.core else { return }
                    planner.core = core
                    await planner.reload()
                }
                .onChange(of: phase) { _, phase in
                    // shortcut: foreground/local sync only in v1; add BGAppRefreshTask before background fetch or push.
                    Task {
                        await model.setForeground(phase == .active)
                        if phase == .active { await planner.reload() }
                    }
                }
        }
    }
}

/// The desktop's list and reading pane side by side on iPad; on a phone the list pushes the message.
struct MailView: View {
    @Bindable var model: MailModel
    let planner: PlannerModel
    @Binding var addAccount: Bool
    @Binding var removing: MailAccount?
    @Environment(\.horizontalSizeClass) private var width
    @Environment(\.verticalSizeClass) private var height
    @State private var retrying: SendFailure?
    @State private var unreadOnly = false

    private var regular: Bool { width == .regular && height == .regular }

    var body: some View {
        Group {
            if regular {
                HStack(spacing: 0) {
                    inbox.frame(width: 360)
                    Theme.line.frame(width: 1)
                    detail.frame(maxWidth: .infinity)
                }
            } else {
                NavigationStack {
                    inbox
                        .toolbar(.hidden, for: .navigationBar)
                        .navigationDestination(isPresented: Binding(get: { model.selectedID != nil }, set: { if !$0 { model.selectedID = nil } })) {
                            detail.toolbar(.hidden, for: .navigationBar)
                        }
                }
            }
        }
        .background(Theme.background)
        .sheet(item: $model.composeDraft) { draft in ComposeView(model: model, draft: draft) }
        .confirmationDialog("Send again? Check Sent first if delivery was uncertain. This can send a duplicate.", isPresented: Binding(get: { retrying != nil }, set: { if !$0 { retrying = nil } })) {
            if let failure = retrying {
                Button("Send again") {
                    retrying = nil
                    Task {
                        do { try await model.core?.retrySend(id: failure.id); await model.sync() }
                        catch { model.error = error.localizedDescription }
                    }
                }
            }
        }
        .task(id: model.selectedID) { await model.select(model.selectedID) }
    }

    /// The desktop list header: title, a sync mark with what is left, the All/Unread filter.
    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .center, spacing: 8) {
                VStack(alignment: .leading, spacing: 1) {
                    if regular {
                        Text(model.searchText.isEmpty ? title : "Search").lagomFont(.base, .bold).lineLimit(1)
                    } else {
                        mailboxMenu
                    }
                    HStack(spacing: 6) {
                        Icon(model.syncing ? "cloud-syncing" : "cloud-synced", size: 16)
                            .accessibilityLabel(model.syncing ? "Syncing" : "Synced").accessibilityHidden(false)
                        if model.mailbox == .inbox && !model.accounts.isEmpty {
                            Text("\(planner.inboxLeft(account: model.accountID)) left").lagomFont(.small).monospacedDigit()
                        }
                    }
                    .foregroundStyle(Theme.secondary)
                }
                Spacer(minLength: 4)
                if !model.accounts.isEmpty { FilterControl(unreadOnly: $unreadOnly) }
                IconButton(title: "New message", icon: "compose") {
                    model.composeDraft = ComposeDraft(account: model.accountID ?? model.accounts.first?.id ?? "")
                }
                .disabled(model.accounts.isEmpty || model.core == nil || model.preparingCompose)
                .padding(.trailing, -6)
            }
            if !regular { SearchField(text: $model.searchText) }
        }
        .padding(.horizontal, 16).padding(.vertical, 6)
        .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
    }

    /// On a phone the title opens the mailboxes and accounts the iPad sidebar lists.
    private var mailboxMenu: some View {
        Menu {
            Picker("Mailbox", selection: $model.mailbox) {
                ForEach(MailboxChoice.allCases) { Text($0.rawValue).tag($0) }
            }
            Picker("Account", selection: $model.accountID) {
                Text("All accounts").tag(String?.none)
                ForEach(model.accounts, id: \.id) { Text($0.email).tag(Optional($0.id)) }
            }
            Button("Add account") { addAccount = true }
            ForEach(model.accounts, id: \.id) { account in
                Button("Remove \(account.email)", role: .destructive) { removing = account }
            }
        } label: {
            HStack(spacing: 4) {
                Text(model.searchText.isEmpty ? title : "Search").lagomFont(.base, .bold).lineLimit(1)
                Icon("chevron", size: 14).foregroundStyle(Theme.secondary)
            }
            .foregroundStyle(Theme.ink)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("\(model.searchText.isEmpty ? title : "Search"), mailboxes and accounts")
        }
    }

    private var inbox: some View {
        let rows = unreadOnly ? model.rows.filter(\.unread) : model.rows
        let words = DateWords()
        return VStack(spacing: 0) {
            header
            List {
                Group {
                    if let notice = model.notice {
                        Text(notice).lagomFont(.snippet).foregroundStyle(Theme.secondary).padding(10)
                            .onTapGesture { model.notice = nil }
                    }
                    if !model.failures.isEmpty {
                        GroupHeader(title: "Outbox needs attention")
                        ForEach(model.failures, id: \.id) { failure in
                            Button { retrying = failure } label: {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(failure.subject).lagomFont(.base)
                                    Text(failure.message).lagomFont(.small).foregroundStyle(Theme.danger)
                                }
                                .padding(.horizontal, 10).padding(.vertical, 8)
                                .frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityHint("Sends again")
                        }
                    }
                    ForEach(grouped(rows) { words.group(Date(timeIntervalSince1970: TimeInterval($0.dateUnix))) }) { group in
                        GroupHeader(title: group.title)
                        ForEach(group.items, id: \.id) { row in mailRow(row, words: words) }
                    }
                    if model.hasMore {
                        HouseButton(title: "Load more", icon: "load-more") { Task { await model.reload(more: true) } }
                            .frame(maxWidth: .infinity).padding(.vertical, 8)
                    }
                }
                .listRowInsets(EdgeInsets(top: 0, leading: 6, bottom: 0, trailing: 6))
                .listRowSeparator(.hidden)
                .listRowBackground(Color.clear)
            }
            .listStyle(.plain)
            .environment(\.defaultMinListRowHeight, 0)
            .scrollContentBackground(.hidden)
            .refreshable { await model.sync() }
            .overlay {
                if unreadOnly && rows.isEmpty && !model.rows.isEmpty {
                    EmptyState(icon: "read", title: "No unread mail")
                } else if model.rows.isEmpty && model.failures.isEmpty && !model.syncing {
                    EmptyState(icon: "inbox", title: model.accounts.isEmpty ? "Welcome to lagom" : "No mail here",
                               detail: model.accounts.isEmpty ? "Add an account to get started." : "Pull to refresh your mail.") {
                        if model.accounts.isEmpty {
                            HouseButton(title: "Add account", icon: "account-add", primary: true) { addAccount = true }
                        }
                    }
                }
            }
        }
        .background(Theme.background)
    }

    private func mailRow(_ row: MailRow, words: DateWords) -> some View {
        Button { model.selectedID = row.id } label: {
            MailRowView(row: row, accent: accountAccent(model.accounts, row.account),
                        selected: regular && model.selectedID == row.id, words: words)
        }
        .buttonStyle(.plain)
        .contextMenu {
            Button(row.unread ? "Mark read" : "Mark unread") {
                Task { await model.setRead(id: row.id, read: row.unread) }
            }.disabled(model.working)
        }
        .swipeActions(edge: .leading) {
            Button { Task { await model.act(.done, id: row.id) } } label: { Label("Done", image: "done") }.tint(Theme.done)
        }
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            if model.mailbox != .trash {
                Button(role: .destructive) { Task { await model.act(.delete, id: row.id) } } label: { Label("Delete", image: "trash") }
                    .tint(Theme.danger)
            }
            Button { Task { await model.act(.later, id: row.id) } } label: { Label("Later", image: "later") }.tint(Theme.later)
        }
    }

    private var title: String {
        guard model.mailbox == .inbox, let id = model.accountID, let account = model.accounts.first(where: { $0.id == id }) else { return model.mailbox.rawValue }
        return account.displayName
    }

    /// The desktop reading pane's toolbar over the message: Later, Delete, Mark read, and Done filled.
    @ViewBuilder private var detail: some View {
        if let message = model.message {
            let id = message.row.id
            VStack(spacing: 0) {
                // Labels where they fit, as on desktop; icons alone in a narrow pane.
                ViewThatFits(in: .horizontal) {
                    actions(message, iconOnly: !regular)
                    actions(message, iconOnly: true)
                }
                .padding(.horizontal, 16).frame(minHeight: 56)
                .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
                MessageView(message: message, model: model).id(id)
            }
            .background(Theme.background)
            .disabled(model.working)
        } else if regular {
            EmptyState(icon: "mail", title: "No message selected")
        } else {
            // While the message loads, or when it failed to: only the way back.
            VStack(alignment: .leading, spacing: 0) {
                IconButton(title: "Back", icon: "chevron-left") { model.selectedID = nil }
                    .padding(.horizontal, 10).frame(minHeight: 56)
                Spacer()
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.background)
        }
    }
}

extension MailView {
    /// Back on a phone, Later, Delete, Mark read on iPad, then Done filled at the end.
    func actions(_ message: MailMessage, iconOnly: Bool) -> some View {
        let id = message.row.id
        return HStack(spacing: 8) {
            if !regular {
                IconButton(title: "Back", icon: "chevron-left") { model.selectedID = nil }.padding(.leading, -6)
            }
            HouseButton(title: "Later", icon: "later", iconOnly: iconOnly) { Task { await model.act(.later, id: id) } }
            if model.mailbox != .trash {
                HouseButton(title: "Delete", icon: "trash", iconOnly: iconOnly) { Task { await model.act(.delete, id: id) } }
            }
            if regular {
                HouseButton(title: message.row.unread ? "Mark read" : "Mark unread", icon: message.row.unread ? "read" : "unread",
                            iconOnly: iconOnly) {
                    Task { await model.setRead(id: id, read: message.row.unread) }
                }
            }
            Spacer(minLength: 0)
            HouseButton(title: "Done", icon: "done", primary: true) { Task { await model.act(.done, id: id) } }
        }
    }
}

/// The message page hides the system bar for the desktop toolbar; the edge swipe back stays.
extension UINavigationController: @retroactive UIGestureRecognizerDelegate {
    override open func viewDidLoad() {
        super.viewDidLoad()
        interactivePopGestureRecognizer?.delegate = self
    }

    public func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool { viewControllers.count > 1 }
}
