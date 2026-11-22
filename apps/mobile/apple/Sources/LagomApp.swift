import SwiftUI
import UserNotifications

@main
struct LagomApp: App {
    @UIApplicationDelegateAdaptor(MailAppDelegate.self) private var delegate
    private var model: MailModel { delegate.model }
    @State private var planner = PlannerModel()
    @Environment(\.scenePhase) private var phase
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// "Not now" was chosen once; the intro never asks again. "Allow" ends with the system's answer.
    @AppStorage("lagom.notificationsAsked") private var notificationsAsked = false
    @AppStorage(AppSection.storageKey) private var section: AppSection = .mail
    /// Whether the system still waits for the user's choice; nil until it has said.
    @State private var notificationsUndecided: Bool?

    private var stage: LaunchStage {
        LaunchStage(storeOpen: model.core != nil, hasAccount: !model.accounts.isEmpty,
                    notificationsAsked: notificationsAsked, notificationsUndecided: notificationsUndecided)
    }

    var body: some Scene {
        WindowGroup {
            ZStack {
                switch stage {
                // The launch screen again (LaunchIcon, centred in the safe area), so opening the store shows no blank frame.
                case .loading: Image("LaunchIcon").accessibilityHidden(true)
                    .frame(maxWidth: .infinity, maxHeight: .infinity).background(Theme.background.ignoresSafeArea())
                case .intro, .notifications:
                    OnboardingView(model: model, notifications: stage == .notifications, allow: {
                        // The system asks over this step; the inbox follows its answer. No prompt shown, the step stays.
                        Task { await delegate.requestPermission(); await readNotifications() }
                    }, notNow: { notificationsAsked = true })
                    .transition(.opacity)
                case .app: RootView(model: model, planner: planner).transition(.opacity)
                }
            }
            .animation(Motion.move(reduceMotion), value: stage)
            // After the intro the app opens on Mail's unified Inbox, whatever the last account left behind.
            .onChange(of: stage) { _, stage in
                guard stage == .intro else { return }
                section = .mail
                model.accountID = nil
                model.mailbox = .inbox
                model.searchText = ""
            }
            .alert("lagom", isPresented: Binding(get: { model.error != nil || planner.error != nil },
                                                set: { if !$0 { model.error = nil; planner.error = nil } })) {
                Button("OK") { model.error = nil; planner.error = nil }
            } message: { Text(model.error ?? planner.error ?? "") }
            .tint(Theme.accent)
            // Text without its own size gets the body face at the system's body size.
            .modifier(LagomFont(points: 17, style: .body))
            .preferredColorScheme(Self.debugScheme)
            .task(id: phase) {
                guard phase == .active else { return }
                // The store opens first: a slow notification-settings answer must never hold back the mail.
                async let settings: Void = readNotifications()
                await model.start()
                await settings
            }
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

    /// DEBUG `-LagomTheme dark|light` forces an appearance for screenshots without touching the device's setting,
    /// as Android's `lagomTheme` extra does; otherwise the system decides.
    private static var debugScheme: ColorScheme? {
        #if DEBUG
        switch UserDefaults.standard.string(forKey: "LagomTheme") {
        case "dark": .dark
        case "light": .light
        default: nil
        }
        #else
        nil
        #endif
    }

    private func readNotifications() async {
        // An answer that never comes must not keep the window blank: after 2 s the app opens, and the
        // notifications step waits for a later launch. Seen on an iOS 27.2 simulator, 2026-11-15.
        let wait = Task { try? await Task.sleep(for: .seconds(2)); if notificationsUndecided == nil { notificationsUndecided = false } }
        let status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
        wait.cancel()
        notificationsUndecided = status == .notDetermined
    }
}

/// The desktop's list and reading pane side by side on iPad; on a phone the list pushes the message.
struct MailView: View {
    @Bindable var model: MailModel
    let planner: PlannerModel
    @Binding var settingsOpen: Bool
    @Environment(\.horizontalSizeClass) private var width
    @Environment(\.verticalSizeClass) private var height
    @State private var retrying: SendFailure?
    /// The message "Move to" asks a folder for, with its account.
    @State private var moving: (account: String, id: Int64)?
    @State private var newFolder: NewFolderTarget?
    @State private var confirmEmptyTrash = false
    // The filter outlives a visit to another page, as on desktop.
    @SceneStorage("lagom.unreadOnly") private var unreadOnly = false
    @State private var paneWidth = CGFloat.infinity
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.railInset) private var railInset
    @Environment(\.pageMargin) private var pageMargin
    /// Rows Done, Later or Delete just took: they leave at once and come back only if the core refuses.
    @State private var leaving: Set<Int64> = []
    /// The row ids the list last drew, to tell a few arriving rows from a whole page loading.
    @State private var drawnIDs: [Int64] = []
    /// Counts committed swipes and toolbar actions, for the haptic.
    @State private var acted = 0
    /// The sync mark turns to "syncing" only once a sync passes `Motion.calm`.
    @State private var syncShown = false
    /// Search is a button until tapped; the field stays while it holds text.
    @State private var searching = false
    @State private var focusOnOpen = false
    @FocusState private var searchFocused: Bool

    private var regular: Bool { width == .regular && height == .regular }
    /// The iPad sidebar holds Settings; an unfolded Duo is regular too but has no sidebar.
    private var sidebar: Bool { regular && UIDevice.current.userInterfaceIdiom == .pad }
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
        // Below the Duo's status bar (about 150 pt) and centred on its tab bar, which sits 6 pt in from the
        // column's centre; both measured in the Duo simulator.
        // The tab bar hides while a message is open, so Delete and Done take its place at the bottom.
        .railColumn(top: { railTop }, bottom: { railBottom })
        .safeAreaInset(edge: .bottom) {
            // The desktop toast: a raised card over the list that rises on the spring; its text changes in place.
            ZStack {
                if let notice = model.notice {
                    HStack(spacing: 12) {
                        Text(notice).lagomFont(.base, .medium).foregroundStyle(Theme.ink).lineLimit(3).padding(.vertical, 8)
                            .contentTransition(.opacity)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        if model.undoSendID != nil {
                            Button("Undo") { Task { await model.undoSend() } }
                                .padding(.horizontal, 8).frame(minWidth: 44, minHeight: 44).accessibilityLabel("Undo send")
                                .disabled(model.composeBusy || model.preparingCompose)
                        } else if !model.undoChanges.isEmpty {
                            Button("Undo") { Task { await model.undoAction() } }
                                .padding(.horizontal, 8).frame(minWidth: 44, minHeight: 44).disabled(model.working)
                        } else {
                            Button("Dismiss") { model.notice = nil }.padding(.horizontal, 8).frame(minWidth: 44, minHeight: 44)
                        }
                    }
                    // The buttons read in the label's size; the label keeps its own weight.
                    .lagomFont(.base, .semibold).buttonStyle(.borderless)
                    .padding(.leading, 14).padding(.trailing, 6)
                    .background(Theme.raised, in: RoundedRectangle(cornerRadius: Theme.Radius.panel))
                    .overlay(RoundedRectangle(cornerRadius: Theme.Radius.panel).strokeBorder(Theme.line))
                    .shadow(color: .black.opacity(0.12), radius: 12, y: 4)
                    .padding(.horizontal, 12).padding(.bottom, 8)
                    .accessibilityElement(children: .contain)
                    .transition(Motion.slide(.bottom, reduceMotion))
                }
            }
            .animation(Motion.move(reduceMotion), value: model.notice)
        }
        .sensoryFeedback(.impact(weight: .light), trigger: acted)
        .background(Theme.background)
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { paneWidth = $0 }
        .sheet(item: $model.composeDraft) { draft in ComposeView(model: model, draft: draft).duoRail() }
        .task(id: model.rows.count) { await model.openDemoCompose() }
        .confirmationDialog("Move to", isPresented: Binding(get: { moving != nil }, set: { if !$0 { moving = nil } }), presenting: moving) { row in
            ForEach(model.moveTargets(account: row.account), id: \.id) { folder in
                Button(folder.path) { act(.move(folder), id: row.id) }
            }
            // Desktop's Move to offers "New folder" too; the message then moves there.
            Button("New folder…") { newFolder = NewFolderTarget(account: row.account, move: row.id) }.disabled(model.working)
        }
        .modifier(NewFolderAlert(target: $newFolder, model: model) { folder, id in act(.move(folder), id: id) })
        // Desktop's words: the subtitle asks, the button deletes.
        .confirmationDialog("Delete all permanently?", isPresented: $confirmEmptyTrash, titleVisibility: .visible) {
            Button("Delete all", role: .destructive) { Task { await model.emptyTrash() } }
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
        .task(id: model.selectedID) { await model.select(model.selectedID) }
        .sheet(isPresented: $settingsOpen) { SettingsView(model: model, sheet: true).duoRail() }
    }

    private var composeButton: some View {
        IconButton(title: "New message", icon: "compose") {
            let account = model.accountID ?? model.accounts.first?.id ?? ""
            Task { await model.newMessage(account: account) }
        }
        .disabled(model.accounts.isEmpty || model.core == nil || model.preparingCompose)
    }

    /// The Duo's right column: New message over the list; with a message open, Back, Reply,
    /// Later and More at the top, Done at the bottom.
    private func railButton(_ title: String, icon: String, primary: Bool = false, selected: Bool = false, action: @escaping () -> Void) -> some View {
        RailButton(title: title, icon: icon, primary: primary, selected: selected, action: action)
    }

    @ViewBuilder private var railTop: some View {
        if let id = model.selectedID {
            // A message opened from a notification can be outside the loaded page; its own row stands in.
            let row = model.rows.first { $0.id == id || $0.messageIds.contains(id) } ?? model.message?.row
            VStack(spacing: 4) {
                // Back first, under the status bar, though a swipe goes back too (Alex 2026-11-17).
                railButton("Back", icon: "chevron-left") { model.selectedID = nil }.padding(.bottom, 8)
                RailGroup {
                railButton("Reply", icon: "reply") { Task { await model.prepareCompose(id: id, kind: .reply) } }
                railButton("Later", icon: "later") { act(.later, id: id) }.contextMenu { laterTimes(id: id) }
                if let row {
                    Menu { moreItems(row, id: id) } label: {
                        Icon("more", label: "More").frame(width: 48, height: 48).contentShape(Circle())
                    }
                    // The same order wherever the menu opens, as on Mac and Android.
                    .menuOrder(.fixed)
                    .foregroundStyle(Theme.ink)
                }
                }
            }
            .disabled(model.working || model.preparingCompose)
        } else {
            // The filter stays in the list header beside the mailbox: it sorts the list, it is not an action (Alex 2026-11-17).
            railButton("New message", icon: "compose") {
                let account = model.accountID ?? model.accounts.first?.id ?? ""
                Task { await model.newMessage(account: account) }
            }
            .disabled(model.accounts.isEmpty || model.core == nil || model.preparingCompose)
        }
    }

    @ViewBuilder private var railBottom: some View {
        if let id = model.selectedID {
            railButton("Done", icon: "done", primary: true) { act(.done, id: id) }
                .disabled(model.working)
        }
    }

    /// The list header (Alex 2026-11-20): the lagom mark with the mailbox on the right, then the filter with
    /// search and New message on the right. The iPad keeps the mailbox at the left, under its sidebar's brandless list.
    private var header: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                if !sidebar {
                    Image("LaunchIcon").resizable().frame(width: 28, height: 28).accessibilityHidden(true)
                    Spacer(minLength: 8)
                }
                mailboxMenu
                if sidebar { Spacer(minLength: 0) }
            }
            .frame(minHeight: 44)
            if searchOpen {
                HStack(spacing: 8) {
                    SearchField(text: $model.searchText).focused($searchFocused)
                    Button("Cancel") { model.searchText = ""; searching = false }.frame(minHeight: 44)
                }
                // The keyboard rises once, with the field the button opened, not each time a kept search reappears.
                .onAppear { if focusOnOpen { searchFocused = true; focusOnOpen = false } }
            } else {
                // At the largest text sizes the buttons go under the filter, so nothing collapses to "…".
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 8) { listControl; Spacer(minLength: 8); headerButtons }
                    VStack(alignment: .leading, spacing: 4) { listControl; HStack(spacing: 8) { Spacer(minLength: 0); headerButtons } }
                }
            }
        }
        .pageEdges().padding(.bottom, 6)
        .animation(Motion.move(reduceMotion), value: searchOpen)
        // An empty field closes when it loses focus, or when a jump to the Inbox or a folder clears the search.
        .onChange(of: searchFocused) { _, focused in if !focused && model.searchText.isEmpty { searching = false } }
        .onChange(of: model.searchText) { _, text in if text.isEmpty && !searchFocused { searching = false } }
    }

    /// The filter, or Empty Trash in Trash, as on desktop.
    @ViewBuilder private var listControl: some View {
        if model.mailbox == .trash && model.folder == nil && !model.accounts.isEmpty {
            HouseButton(title: model.emptyingTrash ? "Emptying…" : "Empty Trash", icon: "empty-trash") { confirmEmptyTrash = true }
                .disabled(model.rows.isEmpty || model.working)
        } else if !model.accounts.isEmpty && model.mailbox != .drafts { FilterControl(unreadOnly: $unreadOnly) }
    }

    /// Search, a button that opens the field as on the Mac (Alex 2026-11-20), and New message without a rail.
    /// iOS 26's minimized `.searchable` draws nothing here: the bar it lives in is hidden for lagom's own header.
    @ViewBuilder private var headerButtons: some View {
        HStack(spacing: 0) {
            if model.mailbox != .drafts {
                IconButton(title: "Search", icon: "search") { searching = true; focusOnOpen = true }
            }
            if railInset == 0 { composeButton }
        }
        // The last glyph ends on the mailbox title's right edge (measured on the folded Duo).
        .padding(.trailing, -14)
    }

    private var searchOpen: Bool { model.mailbox != .drafts && (searching || !model.searchText.isEmpty) }

    /// The title, sync mark and what is left open the mailboxes and accounts; the account filter
    /// also narrows Later, Sent, Done and Trash.
    private var mailboxMenu: some View {
        let name = model.mailbox == .drafts || model.searchText.isEmpty ? title : "Search"
        let left = model.mailbox == .inbox && model.folder == nil && !model.accounts.isEmpty ? planner.inboxLeft(account: model.accountID) : nil
        let status = syncStatus(state: syncShown && model.syncing ? .syncing : model.syncState, remaining: left)
        return Menu {
            // No mailbox shows as chosen while a folder is open.
            Picker("Mailbox", selection: Binding(get: { model.folder == nil ? Optional(model.mailbox) : nil }, set: { if let box = $0 { model.mailbox = box } })) {
                ForEach(MailboxChoice.allCases) { Text($0.rawValue).tag(Optional($0)) }
            }
            ForEach(model.accounts.filter { model.accountID == nil || $0.id == model.accountID }, id: \.id) { account in
                Section(model.accounts.count > 1 ? account.displayName : "Folders") {
                    ForEach(model.folders.filter { $0.account == account.id }, id: \.id) { folder in
                        Button { model.open(folder) } label: {
                            if model.folder?.id == folder.id { Label(folder.path, systemImage: "checkmark") } else { Text(folder.path) }
                        }
                    }
                    Button("New folder") { newFolder = NewFolderTarget(account: account.id) }.disabled(model.working)
                }
            }
            Picker("Account", selection: Binding(get: { model.accountID }, set: { model.folder = nil; model.accountID = $0 })) {
                Text("All accounts").tag(String?.none)
                ForEach(model.accounts, id: \.id) { Text($0.email).tag(Optional($0.id)) }
            }
            // The menu is a phone's sidebar, so Settings closes it as on the Mac; accounts and their names,
            // signatures and removal live there (Alex 2026-11-20).
            if !sidebar {
                Section { Button { settingsOpen = true } label: { Label { Text("Settings") } icon: { Image("settings") } } }
            }
        } label: {
            VStack(alignment: sidebar ? .leading : .trailing, spacing: 1) {
                HStack(spacing: 4) {
                    Text(name).font(.custom(Theme.Face.display, size: TextSize.base.points, relativeTo: .body)).lineLimit(1)
                    Icon("chevron", size: 14).foregroundStyle(Theme.secondary)
                }
                .foregroundStyle(Theme.ink)
                HStack(spacing: 6) {
                    // Synced is the quiet default: the cloud shows only while it syncs or fails.
                    if status.icon != "cloud-synced" { Icon(status.icon == "cloud-failed" ? "cloud-failed" : "cloud-syncing", size: 16) }
                    Text(status.text).lagomFont(.small).monospacedDigit().lineLimit(2).multilineTextAlignment(sidebar ? .leading : .trailing)
                        .contentTransition(.numericText()).animation(reduceMotion ? nil : Motion.fade, value: left)
                }
                .foregroundStyle(Theme.secondary)
            }
            .frame(minHeight: 44).contentShape(Rectangle())
            .accessibilityElement(children: .ignore)
            .accessibilityLabel([name, status.text, "mailboxes and accounts"].joined(separator: ", "))
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
                                Text(Date(timeIntervalSince1970: Double(draft.updatedUnix)), style: .date).lagomFont(.small)
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
        // Trash shows Empty Trash instead of the filter, so it lists everything Empty Trash deletes. A search replaces
        // the filter, so it finds read mail too; the filter comes back with the mailbox.
        let unreadOnly = unreadOnly && !(model.mailbox == .trash && model.folder == nil) && model.searchQuery.isEmpty
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
                .listRowInsets(EdgeInsets(top: 0, leading: pageMargin - 10, bottom: 0, trailing: pageMargin - 10))
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
                    } else if model.rows.isEmpty && model.failures.isEmpty && !model.synced {
                        // Until the first sync lands, quiet placeholder rows stand in, not inbox zero (desktop #141).
                        LoadingRows()
                    } else if model.rows.isEmpty && model.failures.isEmpty {
                        if !model.searchQuery.isEmpty {
                            EmptyState(icon: "search", title: "No results", detail: "Nothing in your mail matches.")
                        } else if model.folder != nil {
                            EmptyState(icon: "inbox", title: "No mail here", detail: "Pull to refresh your mail.")
                        } else {
                            // Inbox zero, as on desktop: the inbox icon, one line, no confetti. Each other mailbox has its own line.
                            EmptyState(icon: model.mailbox.icon, title: model.mailbox.empty.title, detail: model.mailbox.empty.detail)
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
            Button("Move to…") { moving = (row.account, row.id) }.disabled(model.working)
        }
        // How far the row is dragged picks Done, Later or Delete, as on desktop; no small buttons to aim at.
        .modifier(StageSwipe(canDelete: model.mailbox != .trash, enabled: !model.working) { stage in
            switch stage {
            case .done: act(.done, id: row.id)
            case .later: act(.later, id: row.id)
            case .delete: act(.delete, id: row.id)
            }
        })
        // The swipe's moves for VoiceOver, which the drag cannot reach.
        .accessibilityActions {
            Button("Done") { act(.done, id: row.id) }
            Button("Later") { act(.later, id: row.id) }
            if model.mailbox != .trash { Button("Delete") { act(.delete, id: row.id) } }
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
        if let folder = model.folder { return folder.name }
        guard model.mailbox == .inbox, let id = model.accountID, let account = model.accounts.first(where: { $0.id == id }) else { return model.mailbox.rawValue }
        return account.displayName
    }

    /// The reading pane's toolbar over the message: Reply, Later, More, and Done filled, as on every platform.
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
                // On the Duo every action, Back too, sits in the right column.
                if railInset == 0 {
                    // Labels where they fit, as on desktop; icons alone in a narrow pane.
                    ViewThatFits(in: .horizontal) {
                        actions(message, iconOnly: !regular)
                        actions(message, iconOnly: true)
                    }
                    .padding(.horizontal, 16).frame(minHeight: 56)
                }
                ConversationView(model: model).id(id)
            }
            .background(Theme.background)
            .disabled(model.working)
        } else if let id = model.selectedID, let row = model.rows.first(where: { $0.id == id || $0.messageIds.contains(id) }) {
            // While the message loads its list preview stands in, under a bar as tall as the toolbar. On the Duo the
            // loaded message has no toolbar, so the preview has none either and the subject does not jump.
            VStack(spacing: 0) {
                if railInset == 0 {
                    HStack {
                        if !split { IconButton(title: "Back", icon: "chevron-left") { model.selectedID = nil } }
                    }
                    .padding(.horizontal, 10).frame(maxWidth: .infinity, minHeight: 56, alignment: .leading)
                }
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
    /// Back when the message is pushed, then Reply, Later and More, and Done filled at the end: the same set and
    /// order as Mac and Android. Later is one tap (tomorrow, as desktop's L); a long press offers the other times.
    func actions(_ message: MailMessage, iconOnly: Bool) -> some View {
        let id = model.selectedID ?? message.row.id
        return HStack(spacing: 8) {
            if !split {
                IconButton(title: "Back", icon: "chevron-left") { model.selectedID = nil }.padding(.leading, -6)
            }
            HouseButton(title: "Reply", icon: "reply", iconOnly: iconOnly) { Task { await model.prepareCompose(id: id, kind: .reply) } }
                .disabled(model.preparingCompose)
            HouseButton(title: "Later", icon: "later", iconOnly: iconOnly) { act(.later, id: id) }
                .contextMenu { laterTimes(id: id) }
            Menu { moreItems(message.row, id: id) } label: { ButtonLabel(title: "More", icon: "more", iconOnly: true) }
                .menuStyle(.button).menuOrder(.fixed).buttonStyle(HouseButtonStyle())
            Spacer(minLength: 0)
            HouseButton(title: "Done", icon: "done", primary: true) { act(.done, id: id) }
        }
    }

    /// The overflow, in the same order on every platform: Reply all, Forward, Move, Delete, Mark unread.
    @ViewBuilder func moreItems(_ row: MailRow, id: Int64) -> some View {
        Button { Task { await model.prepareCompose(id: id, kind: .replyAll) } } label: { Label("Reply all", image: "reply-all") }
        Button { Task { await model.prepareCompose(id: id, kind: .forward) } } label: { Label("Forward", image: "forward") }
        Button { moving = (row.account, id) } label: { Label("Move", image: "move") }
        if model.mailbox != .trash { Button { act(.delete, id: id) } label: { Label("Delete", image: "trash") } }
        // The row's flag, so the row's members: on the Duo rail it is the grouped list row, not only the selected message.
        Button { Task { await model.setRead(row: row) } } label: {
            Label(row.unread ? "Mark read" : "Mark unread", image: row.unread ? "read" : "unread")
        }
    }

    /// Later's other times, behind a long press.
    @ViewBuilder func laterTimes(id: Int64) -> some View {
        // The menu is built ahead of the press, so read the time again when the item fires.
        ForEach(Array((model.core?.laterChoices() ?? []).enumerated()), id: \.offset) { ix, choice in
            Button(choice.label) {
                let choices = model.core?.laterChoices() ?? []
                act(.later, id: id, until: choices.indices.contains(ix) ? choices[ix].untilUnix : choice.untilUnix)
            }
        }
    }
}

/// Where "New folder" puts its folder, and the message Move to then moves there.
struct NewFolderTarget {
    let account: String
    var move: Int64?
}

/// Desktop's "New folder": the name field and Create. Core checks the name; a refusal shows as an error.
struct NewFolderAlert: ViewModifier {
    @Binding var target: NewFolderTarget?
    let model: MailModel
    var created: (MailFolder, Int64) -> Void = { _, _ in }
    @State private var name = ""

    func body(content: Content) -> some View {
        content.alert("New folder", isPresented: Binding(get: { target != nil }, set: { if !$0 { target = nil } }), presenting: target) { target in
            TextField("Folder name", text: $name)
            Button("Create") {
                let typed = name
                name = ""
                Task {
                    guard let folder = await model.createFolder(typed, account: target.account) else { return }
                    if let id = target.move { created(folder, id) }
                }
            }
            Button("Cancel", role: .cancel) { name = "" }
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
