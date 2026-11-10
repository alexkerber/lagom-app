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

struct MailView: View {
    @Bindable var model: MailModel
    let planner: PlannerModel
    @Binding var section: AppSection
    @Binding var addAccount: Bool
    @Binding var removing: MailAccount?
    @Environment(\.horizontalSizeClass) private var width
    @Environment(\.verticalSizeClass) private var height
    @State private var retrying: SendFailure?
    @State private var unreadOnly = false

    var body: some View {
        Group {
            if width == .regular && height == .regular {
                NavigationSplitView {
                    SidebarView(model: model, planner: planner, section: $section, addAccount: $addAccount, removing: $removing)
                } content: {
                    inbox
                } detail: {
                    detail
                }
                .navigationSplitViewStyle(.balanced)
            } else {
                NavigationSplitView {
                    inbox
                } detail: {
                    detail
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

    private var inbox: some View {
        let rows = unreadOnly ? model.rows.filter(\.unread) : model.rows
        let words = DateWords()
        return List(selection: $model.selectedID) {
            if !model.accounts.isEmpty {
                HStack {
                    if model.mailbox == .inbox {
                        Text("\(planner.inboxLeft(account: model.accountID)) left").font(.subheadline).foregroundStyle(Theme.secondary)
                    }
                    Spacer()
                    Picker("Show", selection: $unreadOnly) {
                        Text("All").tag(false)
                        Text("Unread").tag(true)
                    }
                    .pickerStyle(.segmented).fixedSize()
                }
                .listRowSeparator(.hidden).listRowBackground(Theme.background)
            }
            if let notice = model.notice {
                Text(notice).font(.callout).foregroundStyle(Theme.secondary)
                    .onTapGesture { model.notice = nil }
            }
            if !model.failures.isEmpty {
                Section("Outbox needs attention") {
                    ForEach(model.failures, id: \.id) { failure in
                        Button { retrying = failure } label: {
                            VStack(alignment: .leading) {
                                Text(failure.subject)
                                Text(failure.message).font(.caption).foregroundStyle(Theme.danger)
                            }
                        }
                    }
                }
            }
            ForEach(grouped(rows) { words.group(Date(timeIntervalSince1970: TimeInterval($0.dateUnix))) }) { group in
                Section {
                    ForEach(group.items, id: \.id) { row in
                        NavigationLink(value: row.id) { MailRowView(row: row, accent: accountAccent(model.accounts, row.account), words: words) }
                            .listRowBackground(Theme.background)
                            .swipeActions(edge: .leading) {
                                Button { Task { await model.act(.done, id: row.id) } } label: { Label("Done", systemImage: "checkmark") }.tint(Theme.done)
                            }
                            .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                                if model.mailbox != .trash {
                                    Button(role: .destructive) { Task { await model.act(.delete, id: row.id) } } label: { Label("Delete", systemImage: "trash") }
                                        .tint(Theme.danger)
                                }
                                Button { Task { await model.act(.later, id: row.id) } } label: { Label("Later", systemImage: "clock") }.tint(Theme.later)
                            }
                    }
                } header: {
                    Text(group.title).font(.footnote.weight(.semibold)).foregroundStyle(Theme.secondary).textCase(nil)
                }
            }
            if model.hasMore { Button("Load more") { Task { await model.reload(more: true) } } }
        }
        .listStyle(.plain)
        .overlay {
            if unreadOnly && rows.isEmpty && !model.rows.isEmpty {
                ContentUnavailableView("No unread mail", systemImage: "envelope.open")
            } else if model.rows.isEmpty && model.failures.isEmpty && !model.syncing {
                ContentUnavailableView {
                    Label(model.accounts.isEmpty ? "Welcome to lagom" : "No mail here", systemImage: "tray")
                } description: {
                    Text(model.accounts.isEmpty ? "Add an account to get started." : "Pull to refresh your mail.")
                } actions: {
                    if model.accounts.isEmpty { Button("Add account") { addAccount = true } }
                }
            }
        }
        .navigationTitle(title)
        .refreshable { await model.sync() }
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Menu {
                    Picker("Mailbox", selection: $model.mailbox) {
                        ForEach(MailboxChoice.allCases) { Text($0.rawValue).tag($0) }
                    }
                    Picker("Account", selection: $model.accountID) {
                        Text("All accounts").tag(String?.none)
                        ForEach(model.accounts, id: \.id) { Text($0.email).tag(Optional($0.id)) }
                    }
                    Button("Add account", systemImage: "plus") { addAccount = true }
                    ForEach(model.accounts, id: \.id) { account in
                        Button("Remove \(account.email)", role: .destructive) { removing = account }
                    }
                } label: { Label("Mailboxes and accounts", systemImage: "line.3.horizontal.decrease") }
            }
            ToolbarItem(placement: .topBarTrailing) {
                if model.syncing { ProgressView().accessibilityLabel("Syncing") }
            }
            ToolbarItem(placement: .primaryAction) {
                Button { model.composeDraft = ComposeDraft(account: model.accountID ?? model.accounts.first?.id ?? "") } label: { Label("Compose", systemImage: "square.and.pencil") }
                    .disabled(model.accounts.isEmpty || model.core == nil || model.preparingCompose)
            }
        }
        .scrollContentBackground(.hidden).background(Theme.background)
    }

    private var title: String {
        guard model.mailbox == .inbox, let id = model.accountID, let account = model.accounts.first(where: { $0.id == id }) else { return model.mailbox.rawValue }
        return account.displayName
    }

    @ViewBuilder private var detail: some View {
        if let message = model.message {
            MessageView(message: message)
                .toolbar {
                    ToolbarItemGroup(placement: .bottomBar) {
                        Menu {
                            Button("Reply", systemImage: "arrowshape.turn.up.left") {
                                Task { await model.prepareCompose(id: message.row.id, kind: .reply) }
                            }
                            Button("Reply all", systemImage: "arrowshape.turn.up.left.2") {
                                Task { await model.prepareCompose(id: message.row.id, kind: .replyAll) }
                            }
                            Button("Forward", systemImage: "arrowshape.turn.up.right") {
                                Task { await model.prepareCompose(id: message.row.id, kind: .forward) }
                            }
                        } label: { Label("Reply or forward", systemImage: "arrowshape.turn.up.left") }
                            .disabled(model.preparingCompose)
                        if model.preparingCompose { ProgressView().accessibilityLabel("Preparing message") }
                        Button { Task { await model.act(.later, id: message.row.id) } } label: { Label("Later", systemImage: "clock") }
                        Spacer()
                        Button { Task { await model.act(.done, id: message.row.id) } } label: { Label("Done", systemImage: "checkmark") }
                            .buttonStyle(.borderedProminent).tint(Theme.accent)
                    }
                }
                .disabled(model.working)
        } else {
            ContentUnavailableView("Select a message", systemImage: "envelope")
        }
    }
}
