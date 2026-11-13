import Foundation
import Observation
import UniformTypeIdentifiers

enum MailboxChoice: String, CaseIterable, Identifiable {
    case inbox = "Inbox", later = "Later", sent = "Sent", done = "Done", trash = "Trash", drafts = "Drafts"
    var id: String { rawValue }
    var mailbox: Mailbox {
        switch self {
        case .inbox: .inbox
        case .later: .later
        case .done: .done
        case .sent: .sent
        case .trash: .trash
        case .drafts: .inbox
        }
    }
    /// The desktop icon, `apps/desktop/assets/icons`.
    var icon: String { self == .drafts ? "compose" : rawValue.lowercased() }
}

func draftMessageID(email: String) -> String {
    let domain = email.lastIndex(of: "@").map { String(email[email.index(after: $0)...]) } ?? "localhost"
    return "<\(UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased())@\(domain)>"
}

func safeAttachmentName(_ filename: String) -> String {
    let component = filename.replacingOccurrences(of: "\\", with: "/").components(separatedBy: "/").last ?? ""
    let clean = String(String.UnicodeScalarView(component.unicodeScalars.filter { !CharacterSet.controlCharacters.contains($0) }))
    let name = String(clean.trimmingCharacters(in: .whitespacesAndNewlines).prefix(120)).trimmingCharacters(in: .whitespacesAndNewlines)
    return name.isEmpty || name == "." || name == ".." ? "attachment" : name
}

/// Total size above which compose shows a one-line warning; many servers refuse mail near 25 MB.
let attachmentWarningBytes = 20 * 1024 * 1024

/// Reads a picked file; callers run it off the main thread. Files app URLs are security scoped.
// shortcut: reads the whole file into memory with no cap, add a hard limit if people pick very large videos.
func readAttachment(_ url: URL) throws -> SendAttachment {
    let scoped = url.startAccessingSecurityScopedResource()
    defer { if scoped { url.stopAccessingSecurityScopedResource() } }
    let data = try Data(contentsOf: url)
    let mime = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
    return SendAttachment(filename: safeAttachmentName(url.lastPathComponent), mimeType: mime, data: data)
}

// Same boundaries as core split_recipients; desktop keeps the unfinished token in its input.
func recipientToken(_ value: String, replacingWith recipient: String? = nil) -> String {
    var quoted = false, escaped = false, angle = false
    var comments = 0
    var start = value.startIndex
    for at in value.indices {
        let c = value[at]
        if escaped { escaped = false; continue }
        switch c {
        case "\\" where quoted || comments > 0: escaped = true
        case "\"" where comments == 0: quoted.toggle()
        case "(" where !quoted: comments += 1
        case ")" where !quoted: comments = max(0, comments - 1)
        case "<" where !quoted && comments == 0: angle = true
        case ">" where !quoted && comments == 0: angle = false
        case ",", ";", "\n", "\r", "\r\n":
            if !quoted && !angle && comments == 0 { start = value.index(after: at) }
        default: break
        }
    }
    while start < value.endIndex && value[start].isWhitespace { start = value.index(after: start) }
    if let recipient { return String(value[..<start]) + recipient + ", " }
    return value[start...].trimmingCharacters(in: .whitespacesAndNewlines)
}

struct ComposeDraft: Identifiable, Equatable {
    let id = UUID()
    var messageID: String
    var account: String
    var to = ""
    var cc = ""
    var subject = ""
    var body = ""
    var replyTo: Int64?
    var attachments: [SendAttachment] = []
    var title = "New message"
    var restored = false
    var isReply = false
    var hasContent: Bool { !to.isEmpty || !cc.isEmpty || !subject.isEmpty || !body.isEmpty || !attachments.isEmpty }
    var attachmentsTooLarge: Bool { attachments.reduce(0) { $0 + $1.data.count } > attachmentWarningBytes }

    init(account: String, email: String = "") { self.account = account; messageID = draftMessageID(email: email) }
    init(reply: ReplyDraft, kind: ComposeKind = .reply, email: String = "") {
        messageID = draftMessageID(email: email)
        account = reply.account
        to = reply.to
        cc = reply.cc
        subject = reply.subject
        body = reply.body
        replyTo = reply.replyTo
        isReply = reply.replyTo != nil
        attachments = reply.attachments
        switch kind {
        case .reply: title = "Reply"
        case .replyAll: title = "Reply all"
        case .forward: title = "Forward"
        }
    }

    init(saved: LocalDraft) {
        messageID = saved.id
        account = saved.account
        to = saved.to; cc = saved.cc; subject = saved.subject; body = saved.body
        attachments = saved.attachments
        restored = true
        isReply = saved.inReplyTo != nil || !saved.references.isEmpty
        title = isReply ? "Reply" : "Draft"
    }

    func save(using core: LagomCore) async throws -> DraftRow {
        try await core.saveDraft(id: messageID, accountId: account, to: to, cc: cc, subject: subject,
                                 body: body, replyTo: replyTo, attachments: attachments)
    }

    func enqueue(using core: LagomCore) async throws {
        _ = try await save(using: core)
        _ = try await core.sendDraft(id: messageID)
    }
}

struct MailTarget: Equatable {
    let account: String
    let message: Int64
}

struct MailNotice: Equatable {
    let target: MailTarget
    let title: String
    let body: String
}

func mailNotices(_ rows: [MailRow]) -> [MailNotice] {
    Dictionary(grouping: rows, by: \.account).sorted { $0.key < $1.key }.compactMap { account, rows in
        let newest = rows.sorted { $0.dateUnix == $1.dateUnix ? $0.id > $1.id : $0.dateUnix > $1.dateUnix }
        guard let first = newest.first else { return nil }
        return MailNotice(target: MailTarget(account: account, message: first.id),
                          title: rows.count == 1 ? (first.senderName.isEmpty ? first.senderAddress : first.senderName) : "\(rows.count) new messages",
                          body: rows.count == 1 ? first.subject : newest.prefix(3).map(\.subject).joined(separator: "\n"))
    }
}

@MainActor @Observable
final class MailModel {
    let attachmentDirectory: URL
    var attachmentURL: URL?
    private(set) var downloadingAttachments: Set<String> = []

    @ObservationIgnored private let sendDelay: @Sendable () async throws -> Void
    /// The app sets this to hold a background task; it returns the release. Tests run without UIKit.
    @ObservationIgnored var keepAlive: @MainActor () -> @MainActor () -> Void = { {} }

    init(sendDelay: @escaping @Sendable () async throws -> Void = { try await Task.sleep(for: .seconds(5)) }, attachmentDirectory: URL = URL.cachesDirectory.appendingPathComponent("attachments", isDirectory: true)) {
        self.attachmentDirectory = attachmentDirectory
        self.sendDelay = sendDelay
    }

    func openAttachment(messageID: Int64, part: MailAttachment) async {
        guard let core else { return }
        let key = "\(messageID):\(part.section)"
        guard downloadingAttachments.insert(key).inserted else { return }
        defer { downloadingAttachments.remove(key) }
        let selection = selectionRevision
        do {
            let data = try await core.attachment(messageId: messageID, section: part.section)
            let url = attachmentDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
                .appendingPathComponent(safeAttachmentName(part.filename))
            try await Task.detached {
                try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
                try data.write(to: url, options: .atomic)
            }.value
            guard selection == selectionRevision, !Task.isCancelled else { return }
            attachmentURL = url
        } catch { if selection == selectionRevision && !Task.isCancelled { self.error = error.localizedDescription } }
    }

    var core: LagomCore?
    var accounts: [MailAccount] = []
    var rows: [MailRow] = []
    var failures: [SendFailure] = []
    var drafts: [DraftRow] = []
    var savedDraftIDs: Set<String> = []
    private(set) var notificationRevision = 0
    var notificationTarget: MailTarget?
    var accountID: String?
    var mailbox: MailboxChoice = .inbox
    var searchText = ""
    var remoteImagesIDs: Set<Int64> = []
    var messages: [MailMessage] = []
    var expandedIDs: Set<Int64> = []
    var inlineRevision = 0
    private var selectionRevision = 0
    var selectedID: Int64?
    var message: MailMessage?
    var error: String?
    var notice: String?
    /// The reading view's "Unsubscribe from …" line: the run in progress, and what finished this session.
    private(set) var unsubscribingID: Int64?
    private(set) var unsubscribedIDs: Set<Int64> = []
    private(set) var undoSendID: String?
    private var sendRevision = 0
    var composeDraft: ComposeDraft? {
        didSet {
            guard !endingCompose, !composeBusy, oldValue != composeDraft else { return }
            autosaveTask?.cancel()
            if let draft = composeDraft {
                guard draft.hasContent || oldValue?.hasContent == true || savedDraftIDs.contains(draft.messageID) else { return }
                autosaveTask = Task { [weak self] in
                    do { try await Task.sleep(for: .milliseconds(500)) } catch { return }
                    guard !Task.isCancelled else { return }
                    await self?.keep(draft)
                }
            } else if let draft = oldValue {
                // Interactive sheet dismissal must flush its final edit, even before the debounce fires.
                composeDisappeared(draft)
            }
        }
    }
    var composeBusy = false
    private var draftRevision = 0
    private var finishedDraftIDs: Set<String> = []
    private var endingCompose = false
    @ObservationIgnored private var autosaveTask: Task<Void, Never>?
    @ObservationIgnored private var draftWrite: Task<Void, Error>?
    var preparingCompose = false
    var syncing = false
    private var actingID: Int64?
    private var readingIDs: Set<Int64> = []
    var working: Bool { actingID != nil || !readingIDs.isEmpty }
    var hasMore = false
    private var revision = 0
    private var syncAgain = false
    private var foreground = true
    @ObservationIgnored private var wakeTask: Task<Void, Never>?

    deinit { wakeTask?.cancel(); autosaveTask?.cancel() }

    static func openCore() async throws -> LagomCore {
        let directory = try Storage.directory()
        #if os(iOS)
        // Cached mail stays encrypted while locked (unless open) and out of backups. New files inherit the directory's class.
        let files = FileManager.default
        try files.createDirectory(at: directory, withIntermediateDirectories: true)
        for item in [directory] + ((try? files.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []) {
            try files.setAttributes([.protectionKey: FileProtectionType.completeUnlessOpen], ofItemAtPath: item.path)
        }
        var backup = URLResourceValues()
        backup.isExcludedFromBackup = true
        var excluded = directory
        try excluded.setResourceValues(backup)
        #endif
        return try await Task.detached { try LagomCore.open(dataDir: directory.path) }.value
    }

    func start() async {
        guard core == nil else { return }
        do {
            // Best effort: a stale cache must never block opening mail.
            try? FileManager.default.removeItem(at: attachmentDirectory)
            core = try await Self.openCore()
            await reload()
            await sync()
        } catch { self.error = error.localizedDescription }
    }

    func reload(more: Bool = false) async {
        guard let core, !Task.isCancelled else { return }
        revision += 1
        let request = revision
        let offset = more ? UInt32(rows.count) : 0
        let account = accountID
        let draftsOnly = mailbox == .drafts
        let mailbox = mailbox.mailbox
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            let due = try await core.nextSnooze()
            let accounts = try await core.accounts()
            var page: [MailRow] = []
            let count = more ? 100 : max(100, rows.count)
            var hasMore = false
            repeat {
                try Task.checkCancellation()
                let batch = try await draftsOnly ? [] : query.isEmpty
                    ? core.mailbox(accountId: account, mailbox: mailbox, limit: 100, offset: offset + UInt32(page.count))
                    : core.search(accountId: account, query: query, limit: 100, offset: offset + UInt32(page.count))
                page += batch
                hasMore = batch.count == 100
            } while hasMore && page.count < count
            let failures = try await core.sendFailures()
            guard request == revision, !Task.isCancelled else { return }
            self.accounts = accounts
            self.failures = failures
            rows = more ? rows + page.filter { next in !rows.contains { $0.id == next.id } } : page
            self.hasMore = hasMore
            wake(at: due)
        } catch { if request == revision && !Task.isCancelled { self.error = error.localizedDescription } }
        let draftRequest = draftRevision
        do {
            let drafts = try await core.localDrafts()
            guard request == revision, draftRequest == draftRevision, !Task.isCancelled else { return }
            self.drafts = drafts
            savedDraftIDs.formUnion(drafts.map(\.id))
        } catch { if request == revision && !Task.isCancelled { self.error = error.localizedDescription } }
    }

    private func wake(at due: Int64?) {
        wakeTask?.cancel()
        wakeTask = nil
        guard foreground, let due else { return }
        wakeTask = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(max(0, Double(due) - Date().timeIntervalSince1970))) }
            catch { return }
            guard !Task.isCancelled else { return }
            await self?.reload()
        }
    }

    func setForeground(_ active: Bool) async {
        foreground = active
        wakeTask?.cancel()
        wakeTask = nil
        if !active, let draft = composeDraft { await keep(draft) }
        if active {
            await reload()
            await sync()
        }
    }

    func openNotification(_ target: MailTarget) {
        composeDraft = nil
        notificationTarget = target
        notificationRevision += 1
        accountID = target.account
        mailbox = .inbox
        searchText = ""
    }

    func changeScope() async {
        selectionRevision += 1
        remoteImagesIDs = []
        messages = []
        expandedIDs = []
        hasMore = false
        selectedID = nil
        message = nil
        rows = []
        await reload()
        guard !Task.isCancelled, core != nil, let target = notificationTarget,
              target.account == accountID, mailbox == .inbox, searchText.isEmpty else { return }
        notificationTarget = nil
        selectedID = target.message
    }

    func select(_ id: Int64?) async {
        selectionRevision += 1
        let request = selectionRevision
        remoteImagesIDs = []
        messages = []
        expandedIDs = []
        inlineRevision = 0
        message = nil
        guard let core, let id else { return }
        do {
            let loaded = try await core.conversation(messageId: id)
            guard selectedID == id, request == selectionRevision else { return }
            messages = loaded
            message = loaded.first { $0.row.id == id } ?? loaded.last
            expandedIDs = Set(loaded.filter { $0.row.id == id || $0.row.unread || $0.row.id == loaded.last?.row.id }.map { $0.row.id })
            for item in loaded where expandedIDs.contains(item.row.id) {
                guard request == selectionRevision else { return }
                async let inline: Void = loadInline(id: item.row.id, request: request)
                if item.row.unread { await setRead(id: item.row.id, read: true) }
                await inline
            }
        } catch { if selectedID == id, request == selectionRevision { self.error = error.localizedDescription } }
    }

    private func loadInline(id: Int64, request: Int) async {
        guard let core, messages.contains(where: { $0.row.id == id && $0.htmlBody != nil }) else { return }
        do {
            try await core.loadInline(id: id)
            guard request == selectionRevision else { return }
            inlineRevision += 1
        } catch { if request == selectionRevision { self.error = error.localizedDescription } }
    }

    func loadRemoteImages(id: Int64) {
        guard messages.contains(where: { $0.row.id == id }) else { return }
        remoteImagesIDs.insert(id)
    }

    func toggleMessage(_ id: Int64) async {
        guard let item = messages.first(where: { $0.row.id == id }) else { return }
        if expandedIDs.remove(id) != nil { return }
        expandedIDs.insert(id)
        let request = selectionRevision
        async let inline: Void = loadInline(id: id, request: request)
        if item.row.unread { await setRead(id: id, read: true) }
        await inline
    }

    func setRead(row: MailRow) async {
        for id in row.messageIds { await setRead(id: id, read: row.unread) }
    }

    func setRead(id: Int64, read: Bool) async {
        // Demo accounts have no server or password; opening an unread message must not raise a keychain error.
        guard let core, !Storage.demo, actingID != id, !readingIDs.contains(id) else { return }
        readingIDs.insert(id)
        defer { readingIDs.remove(id) }
        do {
            try await core.markRead(id: id, read: read)
            if messages.contains(where: { $0.row.id == id }) {
                let request = selectionRevision
                let updated = try await core.message(id: id)
                if request == selectionRevision {
                    messages = messages.map { $0.row.id == id ? updated : $0 }
                    if message?.row.id == id { message = updated }
                }
            }
        } catch { self.error = error.localizedDescription }
        await reload()
    }

    func sync() async {
        guard let core, !Storage.demo else { return }
        if syncing { syncAgain = true; return }
        syncing = true
        defer { syncing = false }
        repeat {
            syncAgain = false
            do { try await core.syncAll() } catch { self.error = error.localizedDescription }
            await reload()
        } while syncAgain
    }

    /// Only after the person confirmed. A web page is the view's to open; the core never fetches it.
    func unsubscribe(id: Int64) async {
        guard let core, unsubscribingID == nil else { return }
        unsubscribingID = id
        defer { unsubscribingID = nil }
        do {
            try await core.unsubscribe(messageId: id)
            unsubscribedIDs.insert(id)
        } catch { self.error = error.localizedDescription }
    }

    func act(_ action: MailAction, id: Int64, until: Int64? = nil) async {
        guard let core, !working else { return }
        let ids = rows.first { $0.messageIds.contains(id) }?.messageIds ?? [id]
        actingID = id
        defer { actingID = nil }
        do {
            switch action {
            case .done: try await core.done(ids: ids)
            case .later: try await core.later(ids: ids, untilUnix: until ?? core.laterChoices()[1].untilUnix)
            case .delete: try await core.delete(ids: ids)
            }
            if selectedID == id { selectedID = nil; message = nil; messages = []; expandedIDs = []; remoteImagesIDs = [] }
        } catch { self.error = error.localizedDescription }
        await reload()
    }

    func prepareCompose(id: Int64, kind: ComposeKind) async {
        guard let core, !preparingCompose, composeDraft == nil else { return }
        let selection = selectionRevision
        let selected = selectedID
        preparingCompose = true
        defer { preparingCompose = false }
        do {
            _ = try? await draftWrite?.value
            let saved = kind == .reply ? try await core.savedReply(id: id) : nil
            if let saved {
                guard selection == selectionRevision, selected == selectedID, composeDraft == nil else { return }
                restoreDraft(saved)
                return
            }
            let reply = try await core.composeDraft(id: id, kind: kind)
            guard selection == selectionRevision, selected == selectedID, composeDraft == nil else { return }
            composeDraft = ComposeDraft(reply: reply, kind: kind, email: accounts.first { $0.id == reply.account }?.email ?? "")
        } catch { if selection == selectionRevision, selected == selectedID { self.error = error.localizedDescription } }
    }

    func openDraft(_ row: DraftRow) async {
        guard let core, composeDraft == nil, !preparingCompose else { return }
        preparingCompose = true
        defer { preparingCompose = false }
        do {
            _ = try? await draftWrite?.value
            // A list row can predate the final save performed by a dismissed sheet.
            let saved = try await core.localDraft(id: row.id)
            guard composeDraft == nil else { return }
            restoreDraft(saved)
        } catch MailError.NotFound {
            draftRevision += 1
            drafts.removeAll { $0.id == row.id }
            savedDraftIDs.remove(row.id)
        } catch { self.error = error.localizedDescription }
    }

    private func restoreDraft(_ saved: LocalDraft) {
        savedDraftIDs.insert(saved.id)
        finishedDraftIDs.remove(saved.id)
        composeDraft = ComposeDraft(saved: saved)
    }

    // Serialize writes: cancelling a Swift task does not cancel an in-flight Rust SQLite write.
    private func save(_ draft: ComposeDraft) -> Task<Void, Error> {
        let previous = draftWrite
        let task = Task { @MainActor in
            _ = try? await previous?.value
            guard let core, !finishedDraftIDs.contains(draft.messageID), draft.hasContent || savedDraftIDs.contains(draft.messageID) else { return }
            let saved = try await draft.save(using: core)
            draftRevision += 1
            savedDraftIDs.insert(saved.id)
            drafts.removeAll { $0.id == saved.id }
            drafts.insert(saved, at: 0)
        }
        draftWrite = task
        return task
    }

    private func keep(_ draft: ComposeDraft) async {
        guard !composeBusy else { return }
        do { try await save(draft).value }
        catch {
            self.error = error.localizedDescription
            if composeDraft == nil, !finishedDraftIDs.contains(draft.messageID) {
                endingCompose = true
                composeDraft = draft
                endingCompose = false
            }
        }
    }

    func composeDisappeared(_ draft: ComposeDraft) {
        guard !composeBusy, !finishedDraftIDs.contains(draft.messageID) else { return }
        let pending = save(draft)
        Task {
            do { try await pending.value }
            catch {
                self.error = error.localizedDescription
                if composeDraft == nil, !finishedDraftIDs.contains(draft.messageID) {
                    endingCompose = true
                    composeDraft = draft
                    endingCompose = false
                }
            }
        }
    }

    func changeComposeAccount(_ account: String) async throws {
        guard let old = composeDraft, let core, !old.isReply, !old.restored, old.account != account, !composeBusy else { return }
        composeBusy = true
        defer { composeBusy = false }
        autosaveTask?.cancel()
        _ = try? await draftWrite?.value
        var replacement = old
        replacement.messageID = draftMessageID(email: accounts.first { $0.id == account }?.email ?? "")
        replacement.account = account
        if old.hasContent || savedDraftIDs.contains(old.messageID) {
            // Save the replacement first so a failed account change cannot lose the original.
            let saved = try await replacement.save(using: core)
            draftRevision += 1
            drafts.insert(saved, at: 0)
            savedDraftIDs.insert(saved.id)
            try await core.deleteDraft(id: old.messageID)
        }
        draftRevision += 1
        drafts.removeAll { $0.id == old.messageID }
        savedDraftIDs.remove(old.messageID)
        finishedDraftIDs.insert(old.messageID)
        endingCompose = true
        composeDraft = replacement
        endingCompose = false
    }

    func closeCompose(discard: Bool) async throws {
        guard let draft = composeDraft, let core, !composeBusy else { return }
        composeBusy = true
        defer { composeBusy = false }
        autosaveTask?.cancel()
        if discard {
            _ = try? await draftWrite?.value
            try await core.deleteDraft(id: draft.messageID)
            draftRevision += 1
            drafts.removeAll { $0.id == draft.messageID }
            savedDraftIDs.remove(draft.messageID)
        } else { try await save(draft).value }
        endCompose()
    }

    func sendCompose() async throws {
        guard let draft = composeDraft, let core, !composeBusy else { return }
        composeBusy = true
        defer { composeBusy = false }
        autosaveTask?.cancel()
        try await save(draft).value
        _ = try await core.sendDraft(id: draft.messageID)
        draftRevision += 1
        drafts.removeAll { $0.id == draft.messageID }
        savedDraftIDs.remove(draft.messageID)
        endCompose()
        sendRevision += 1
        let sending = sendRevision
        undoSendID = draft.messageID
        notice = "Sending…"
        let delay = sendDelay
        // Background sync never sends, so the mail leaves here even if the user locks the phone inside the Undo window.
        // shortcut: a sync already in flight ends the keep-alive early, upgrade if mail stalls in the outbox after locking.
        let release = keepAlive()
        Task { [weak self] in
            defer { release() }
            do { try await delay() } catch { return }
            guard let self else { return }
            if self.sendRevision == sending, self.undoSendID == draft.messageID { self.undoSendID = nil; self.notice = nil }
            await self.sync()
        }
    }

    func undoSend() async {
        guard let core, let id = undoSendID, composeDraft == nil, !composeBusy, !preparingCompose else { return }
        do {
            composeBusy = true
            defer { composeBusy = false }
            if try await core.undoSend(id: id) {
                draftRevision += 1
                let saved = try await core.localDraft(id: id)
                if composeDraft == nil { restoreDraft(saved) }
                notice = nil
            } else { notice = "Already sent" }
            if undoSendID == id { undoSendID = nil }
        } catch { self.error = error.localizedDescription }
    }

    private func endCompose() {
        if let draft = composeDraft { finishedDraftIDs.insert(draft.messageID) }
        endingCompose = true
        composeDraft = nil
        endingCompose = false
    }

    func remove(_ account: MailAccount) async {
        guard let core else { return }
        do {
            try await core.removeAccount(id: account.id)
            if accountID == account.id { accountID = nil }
            await changeScope()
        } catch { self.error = error.localizedDescription }
    }
}

enum MailAction { case done, later, delete }
