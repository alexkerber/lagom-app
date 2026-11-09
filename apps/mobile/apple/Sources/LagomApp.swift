import SwiftUI

@main
struct LagomApp: App {
    @State private var model = MailModel()
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            MailView(model: model)
                .tint(Theme.accent)
                .task { await model.start() }
                .onChange(of: phase) { _, phase in
                    // shortcut: foreground/local sync only in v1; add BGAppRefreshTask before background fetch or push.
                    if phase == .active { Task { await model.sync() } }
                }
        }
    }
}

struct MailView: View {
    @Bindable var model: MailModel
    @Environment(\.horizontalSizeClass) private var width
    @Environment(\.verticalSizeClass) private var height
    @State private var addAccount = false
    @State private var compose: ComposeDraft?
    @State private var removing: MailAccount?
    @State private var retrying: SendFailure?

    var body: some View {
        Group {
            if width == .regular && height == .regular {
                NavigationSplitView {
                    sidebar
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
        .sheet(isPresented: $addAccount) { AddAccountView(model: model) }
        .sheet(item: $compose) { draft in ComposeView(model: model, draft: draft) }
        .alert("Mail", isPresented: Binding(get: { model.error != nil }, set: { if !$0 { model.error = nil } })) {
            Button("OK") { model.error = nil }
        } message: { Text(model.error ?? "") }
        .confirmationDialog("Remove account and its local mail?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } })) {
            if let account = removing {
                Button("Remove \(account.email)", role: .destructive) { Task { await model.remove(account) }; removing = nil }
            }
        }
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
        .onChange(of: model.accountID) { _, _ in Task { await model.changeScope() } }
        .onChange(of: model.mailbox) { _, _ in Task { await model.changeScope() } }
        .task(id: model.selectedID) { await model.select(model.selectedID) }
    }

    private var sidebar: some View {
        List {
            Section("Mailboxes") {
                ForEach(MailboxChoice.allCases) { box in
                    Button { model.mailbox = box } label: {
                        Label(box.rawValue, systemImage: box.symbol)
                            .foregroundStyle(model.mailbox == box ? Theme.accent : Theme.ink)
                    }
                }
            }
            Section("Accounts") {
                Button("All accounts") { model.accountID = nil }
                ForEach(model.accounts, id: \.id) { account in
                    Button { model.accountID = account.id } label: {
                        Label(account.email, systemImage: model.accountID == account.id ? "checkmark.circle.fill" : "person.crop.circle")
                    }
                    .contextMenu { Button("Remove account", role: .destructive) { removing = account } }
                }
                Button { addAccount = true } label: { Label("Add account", systemImage: "plus") }
            }
        }
        .navigationTitle("Lagom")
        .scrollContentBackground(.hidden).background(Theme.subtle)
    }

    private var inbox: some View {
        List(selection: $model.selectedID) {
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
            ForEach(model.rows, id: \.id) { row in
                NavigationLink(value: row.id) { MailRowView(row: row) }
                    .listRowBackground(Theme.background)
                    .swipeActions(edge: .leading) {
                        Button { Task { await model.act(.done, id: row.id) } } label: { Label("Done", systemImage: "checkmark") }.tint(Theme.done)
                    }
                    .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                        Button(role: .destructive) { Task { await model.act(.delete, id: row.id) } } label: { Label("Delete", systemImage: "trash") }
                        Button { Task { await model.act(.later, id: row.id) } } label: { Label("Later", systemImage: "clock") }.tint(Theme.later)
                    }
            }
            if model.hasMore { Button("Load more") { Task { await model.reload(more: true) } } }
        }
        .overlay {
            if model.rows.isEmpty && model.failures.isEmpty && !model.syncing {
                ContentUnavailableView {
                    Label(model.accounts.isEmpty ? "Welcome to Lagom" : "No mail here", systemImage: "tray")
                } description: {
                    Text(model.accounts.isEmpty ? "Add an account to get started." : "Pull to refresh your mail.")
                } actions: {
                    if model.accounts.isEmpty { Button("Add account") { addAccount = true } }
                }
            }
        }
        .navigationTitle(model.mailbox.rawValue)
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
                Button { compose = ComposeDraft(account: model.accountID ?? model.accounts.first?.id ?? "") } label: { Label("Compose", systemImage: "square.and.pencil") }
                    .disabled(model.accounts.isEmpty || model.core == nil)
            }
        }
        .scrollContentBackground(.hidden).background(Theme.background)
    }

    @ViewBuilder private var detail: some View {
        if let message = model.message {
            MessageView(message: message)
                .toolbar {
                    ToolbarItemGroup(placement: .bottomBar) {
                        Button {
                            Task {
                                do {
                                    if let reply = try await model.core?.replyDraft(id: message.row.id) { compose = ComposeDraft(reply: reply) }
                                } catch { model.error = error.localizedDescription }
                            }
                        } label: { Label("Reply", systemImage: "arrowshape.turn.up.left") }
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
