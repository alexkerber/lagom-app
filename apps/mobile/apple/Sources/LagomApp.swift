import SwiftUI

@main
struct LagomApp: App {
    @UIApplicationDelegateAdaptor(MailAppDelegate.self) private var delegate
    private var model: MailModel { delegate.model }
    @State private var planner = PlannerModel()
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            RootView(model: model, planner: planner)
                .tint(Theme.accent)
                .task(id: phase) {
                    guard phase == .active else { return }
                    await model.start()
                    await delegate.requestPermission()
                }
                .task(id: model.accounts.isEmpty) { await delegate.requestPermission() }
                // Local tasks and events load as soon as the store opens, not after the first sync.
                .task(id: model.core == nil) {
                    guard let core = model.core else { return }
                    planner.core = core
                    await planner.reload()
                }
                .onChange(of: phase) { _, phase in
                    if phase == .background { MailAppDelegate.schedule() }
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
    @State private var laterChoices: [LaterChoice] = []
    @State private var laterID: Int64?
    @State private var retrying: SendFailure?
    @State private var signing: MailAccount?
    // The filter outlives a visit to another page, as on desktop.
    @SceneStorage("lagom.unreadOnly") private var unreadOnly = false
    @State private var paneWidth = CGFloat.infinity
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// Rows Done, Later or Delete just took: they leave at once and come back only if the core refuses.
    @State private var leaving: Set<Int64> = []
    /// The row ids the list last drew, to tell a few arriving rows from a whole page loading.
    @State private var drawnIDs: [Int64] = []
    /// Counts committed swipes and toolbar actions, for the haptic.
    @State private var acted = 0
    /// The sync mark turns to "syncing" only once a sync passes `Motion.calm`.
    @State private var syncShown = false

    private var regular: Bool { width == .regular && height == .regular }
    /// List and reading pane side by side only where both fit; a narrow iPad pushes the message as a phone does.
    private var split: Bool { regular && paneWidth >= 740 }

    var body: some View {
        Group {
            if model.mailbox == .drafts {
                draftList
            } else if split {
                HStack(spacing: 0) {
                    inbox.frame(width: 360)
                    Theme.line.frame(width: 1)
                    detail.frame(maxWidth: .infinity)
                }
            } else {
                NavigationStack {
                    inbox
                        .background(SwipeBack())
                        .toolbar(.hidden, for: .navigationBar)
                        .navigationDestination(isPresented: Binding(get: { model.selectedID != nil }, set: { if !$0 { model.selectedID = nil } })) {
                            detail.toolbar(.hidden, for: .navigationBar)
                        }
                }
            }
        }
        .safeAreaInset(edge: .bottom) {
            // The Sending notice rises from the bottom edge on the spring; its text changes in place.
            ZStack {
                if let notice = model.notice {
                    HStack {
                        Text(notice).lagomFont(.snippet).foregroundStyle(Theme.secondary)
                            .contentTransition(.opacity)
                        Spacer()
                        if model.undoSendID != nil {
                            Button("Undo") { Task { await model.undoSend() } }
                                .frame(minWidth: 44, minHeight: 44).accessibilityLabel("Undo send")
                                .disabled(model.composeBusy || model.preparingCompose)
                        } else if !model.undoChanges.isEmpty {
                            Button("Undo") { Task { await model.undoAction() } }
                                .frame(minWidth: 44, minHeight: 44).disabled(model.working)
                        } else {
                            Button("Dismiss") { model.notice = nil }.frame(minWidth: 44, minHeight: 44)
                        }
                    }
                    .padding(.horizontal, 10).background(Theme.background)
                    .transition(Motion.slide(.bottom, reduceMotion))
                }
            }
            .animation(Motion.move(reduceMotion), value: model.notice)
        }
        .confirmationDialog("Later", isPresented: Binding(get: { laterID != nil }, set: { if !$0 { laterID = nil } })) {
            ForEach(laterChoices, id: \.label) { choice in
                Button(choice.label) {
                    guard let id = laterID else { return }
                    laterID = nil
                    act(.later, id: id, until: choice.untilUnix)
                }
            }
        }
        .sensoryFeedback(.impact(weight: .light), trigger: acted)
        .background(Theme.background)
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { paneWidth = $0 }
        .sheet(item: $model.composeDraft) { draft in ComposeView(model: model, draft: draft) }
        .sheet(item: $signing) { account in SignatureView(model: model, account: account) }
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
                mailboxMenu
                Spacer(minLength: 4)
                if !model.accounts.isEmpty && model.mailbox != .drafts { FilterControl(unreadOnly: $unreadOnly) }
                IconButton(title: "New message", icon: "compose") {
                    let account = model.accountID ?? model.accounts.first?.id ?? ""
                    Task { await model.newMessage(account: account) }
                }
                .disabled(model.accounts.isEmpty || model.core == nil || model.preparingCompose)
                .padding(.trailing, -6)
            }
            if !regular && model.mailbox != .drafts { SearchField(text: $model.searchText) }
        }
        .padding(.horizontal, 16).padding(.vertical, 6)
        .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
    }

    /// The title, sync mark and what is left open the mailboxes and accounts; the account filter
    /// also narrows Later, Sent, Done and Trash.
    private var mailboxMenu: some View {
        let name = model.mailbox == .drafts || model.searchText.isEmpty ? title : "Search"
        let left = model.mailbox == .inbox && !model.accounts.isEmpty ? "\(planner.inboxLeft(account: model.accountID)) left" : nil
        return Menu {
            Picker("Mailbox", selection: $model.mailbox) {
                ForEach(MailboxChoice.allCases) { Text($0.rawValue).tag($0) }
            }
            Picker("Account", selection: $model.accountID) {
                Text("All accounts").tag(String?.none)
                ForEach(model.accounts, id: \.id) { Text($0.email).tag(Optional($0.id)) }
            }
            Button("Add account") { addAccount = true }
            ForEach(model.accounts, id: \.id) { account in
                Menu(account.email) {
                    Button("Signature") { signing = account }
                    Button("Remove account", role: .destructive) { removing = account }
                }
            }
        } label: {
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 4) {
                    Text(name).lagomFont(.base, .bold).lineLimit(1)
                    Icon("chevron", size: 14).foregroundStyle(Theme.secondary)
                }
                .foregroundStyle(Theme.ink)
                HStack(spacing: 6) {
                    Icon(syncShown ? "cloud-syncing" : "cloud-synced", size: 16)
                    if let left {
                        Text(left).lagomFont(.small).monospacedDigit()
                            .contentTransition(.numericText()).animation(reduceMotion ? nil : Motion.fade, value: left)
                    }
                }
                .foregroundStyle(Theme.secondary)
            }
            .frame(minHeight: 44).contentShape(Rectangle())
            .accessibilityElement(children: .ignore)
            .accessibilityLabel([name, model.syncing ? "Syncing" : "Synced", left, "mailboxes and accounts"].compactMap { $0 }.joined(separator: ", "))
        }
        .task(id: model.syncing) {
            guard model.syncing else { syncShown = false; return }
            guard (try? await Task.sleep(for: Motion.calm)) != nil else { return }
            syncShown = true
        }
    }

    private var draftList: some View {
        VStack(spacing: 0) {
            header
            List {
                let drafts = model.drafts.filter { model.accountID == nil || $0.account == model.accountID }
                if drafts.isEmpty { Text("No drafts").foregroundStyle(Theme.secondary) }
                ForEach(drafts, id: \.id) { draft in
                    Button { Task { await model.openDraft(draft) } } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(draft.subject.isEmpty ? "(No subject)" : draft.subject).fontWeight(.semibold)
                            Text(draft.to).foregroundStyle(Theme.secondary).lineLimit(1)
                            Text(model.accounts.first { $0.id == draft.account }?.email ?? "").foregroundStyle(Theme.secondary)
                            if draft.updatedUnix > 0 {
                                Text(Date(timeIntervalSince1970: Double(draft.updatedUnix)), style: .date).font(.caption)
                            }
                        }
                        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading).contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Edit draft, \(draft.subject.isEmpty ? "No subject" : draft.subject), \(draft.to)")
                }
            }
            .listStyle(.plain)
            .refreshable { await model.reload() }
        }
    }

    private var inbox: some View {
        let rows = (unreadOnly ? model.rows.filter(\.unread) : model.rows)
            .filter { !leaving.contains($0.id) && !$0.messageIds.contains(where: leaving.contains) }
        let ids = rows.map(\.id)
        let words = DateWords()
        return VStack(spacing: 0) {
            header
            List {
                Group {
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
            // New mail grows in and a done row closes its gap; a page or a scope change appears without motion.
            // shortcut: the list keeps UIKit's scroll offset, so rows arriving above a scrolled list push it down; pin the first visible row if that reads as a jump on device.
            .animation(animatesRowChange(from: drawnIDs, to: ids) ? Motion.move(reduceMotion) : nil, value: ids)
            .onChange(of: ids, initial: true) { _, ids in drawnIDs = ids }
            .refreshable { await model.sync() }
            .overlay {
                Group {
                    if unreadOnly && rows.isEmpty && !model.rows.isEmpty {
                        EmptyState(icon: "read", title: "No unread mail")
                    } else if model.rows.isEmpty && model.failures.isEmpty && !model.syncing {
                        if model.accounts.isEmpty {
                            EmptyState(icon: "inbox", title: "Welcome to lagom", detail: "Add an account to get started.") {
                                HouseButton(title: "Add account", icon: "account-add", primary: true) { addAccount = true }
                            }
                        } else if model.mailbox == .inbox && model.searchText.isEmpty {
                            // Inbox zero, as on desktop: one calm illustration, no confetti.
                            EmptyState(icon: "inbox", title: "Nothing left.", illustration: "EmptyTray")
                        } else {
                            EmptyState(icon: "inbox", title: "No mail here", detail: "Pull to refresh your mail.")
                        }
                    }
                }
                // A scope change empties the list for a moment; the empty page waits out a quick load.
                .transition(Motion.calmIn)
            }
        }
        .background(Theme.background)
    }

    private func mailRow(_ row: MailRow, words: DateWords) -> some View {
        Button { model.selectedID = row.id } label: {
            MailRowView(row: row, accent: accountAccent(model.accounts, row.account),
                        selected: split && model.selectedID == row.id, words: words)
        }
        .buttonStyle(.plain)
        .contextMenu {
            Button(row.unread ? "Mark read" : "Mark unread") {
                Task { await model.setRead(row: row) }
            }.disabled(model.working)
        }
        .swipeActions(edge: .leading) {
            Button { act(.done, id: row.id) } label: { Label("Done", image: "done") }.tint(Theme.done)
        }
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            if model.mailbox != .trash {
                Button(role: .destructive) { act(.delete, id: row.id) } label: { Label("Delete", image: "trash") }
                    .tint(Theme.danger)
            }
            Button { act(.later, id: row.id) } label: { Label("Later", image: "later") }.tint(Theme.later)
        }
    }

    /// Done, Later and Delete act at once (docs/motion.md rule 1): a light tap, the message closes and
    /// its row leaves while the core works. A refused action brings the row back with the next reload.
    private func act(_ action: MailAction, id: Int64, until: Int64? = nil) {
        // The model refuses while another action or a read flag is in flight; leave the row where it is.
        guard !model.working else { return }
        acted += 1
        if model.selectedID == id { model.selectedID = nil }
        leaving.insert(id)
        Task {
            await model.act(action, id: id, until: until)
            leaving.remove(id)
        }
    }

    private var title: String {
        guard model.mailbox == .inbox, let id = model.accountID, let account = model.accounts.first(where: { $0.id == id }) else { return model.mailbox.rawValue }
        return account.displayName
    }

    /// The desktop reading pane's toolbar over the message: Later, Delete, Mark read, and Done filled.
    private var detail: some View {
        Group { reading }
            // The message fades in over its preview instead of popping in.
            .animation(Motion.fade, value: model.message == nil)
    }

    @ViewBuilder private var reading: some View {
        if let message = model.message {
            let id = model.selectedID ?? message.row.id
            VStack(spacing: 0) {
                // Labels where they fit, as on desktop; icons alone in a narrow pane.
                ViewThatFits(in: .horizontal) {
                    actions(message, iconOnly: !regular)
                    actions(message, iconOnly: true)
                }
                .padding(.horizontal, 16).frame(minHeight: 56)
                .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
                ConversationView(model: model).id(id)
            }
            .background(Theme.background)
            .disabled(model.working)
        } else if let id = model.selectedID, let row = model.rows.first(where: { $0.id == id || $0.messageIds.contains(id) }) {
            // While the message loads its list preview stands in, under a bar as tall as the toolbar.
            VStack(spacing: 0) {
                HStack {
                    if !split { IconButton(title: "Back", icon: "chevron-left") { model.selectedID = nil } }
                }
                .padding(.horizontal, 10).frame(maxWidth: .infinity, minHeight: 56, alignment: .leading)
                .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
                MessagePreview(row: row)
            }
            .background(Theme.background)
        } else if split {
            EmptyState(icon: "mail", title: "No message selected")
        } else {
            // A message outside the loaded rows, while it loads or when it failed to: only the way back.
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
    /// Back when the message is pushed, Later, Delete, Mark read on iPad, then Done filled at the end.
    func actions(_ message: MailMessage, iconOnly: Bool) -> some View {
        let id = model.selectedID ?? message.row.id
        return HStack(spacing: 8) {
            if !split {
                IconButton(title: "Back", icon: "chevron-left") { model.selectedID = nil }.padding(.leading, -6)
            }
            HouseButton(title: "Later", icon: "later", iconOnly: iconOnly) {
                laterChoices = model.core?.laterChoices() ?? []; laterID = id
            }
            if model.mailbox != .trash {
                HouseButton(title: "Delete", icon: "trash", iconOnly: iconOnly) { act(.delete, id: id) }
            }
            if regular {
                HouseButton(title: message.row.unread ? "Mark read" : "Mark unread", icon: message.row.unread ? "read" : "unread",
                            iconOnly: iconOnly) {
                    Task { await model.setRead(id: id, read: message.row.unread) }
                }
            }
            Spacer(minLength: 0)
            HouseButton(title: "Done", icon: "done", primary: true) { act(.done, id: id) }
        }
    }
}

/// The message page hides the system bar for the desktop toolbar; this keeps the edge swipe back
/// on Mail's own stack, and never during a push or pop, which would freeze navigation.
private struct SwipeBack: UIViewControllerRepresentable {
    func makeUIViewController(context: Context) -> Hook { Hook() }
    func updateUIViewController(_ hook: Hook, context: Context) {}

    final class Hook: UIViewController, UIGestureRecognizerDelegate {
        override func viewDidAppear(_ animated: Bool) {
            super.viewDidAppear(animated)
            navigationController?.interactivePopGestureRecognizer?.delegate = self
        }

        func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
            guard let stack = navigationController else { return false }
            return stack.viewControllers.count > 1 && stack.transitionCoordinator == nil
        }
    }
}
