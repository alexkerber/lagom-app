import Foundation
import Observation

enum MailboxChoice: String, CaseIterable, Identifiable {
    case inbox = "Inbox", later = "Later", sent = "Sent", done = "Done", trash = "Trash"
    var id: String { rawValue }
    var mailbox: Mailbox {
        switch self {
        case .inbox: .inbox
        case .later: .later
        case .done: .done
        case .sent: .sent
        case .trash: .trash
        }
    }
    /// The desktop icon, `apps/desktop/assets/icons`.
    var icon: String { rawValue.lowercased() }
}

struct ComposeDraft: Identifiable {
    let id = UUID()
    var account: String
    var to = ""
    var cc = ""
    var subject = ""
    var body = ""
    var replyTo: Int64?
    var attachments: [SendAttachment] = []
    var title = "New message"

    init(account: String) { self.account = account }
    init(reply: ReplyDraft, kind: ComposeKind = .reply) {
        account = reply.account
        to = reply.to
        cc = reply.cc
        subject = reply.subject
        body = reply.body
        replyTo = reply.replyTo
        attachments = reply.attachments
        switch kind {
        case .reply: title = "Reply"
        case .replyAll: title = "Reply all"
        case .forward: title = "Forward"
        }
    }

    func enqueue(using core: LagomCore) async throws {
        _ = try await core.send(accountId: account, to: splitRecipients(value: to), cc: splitRecipients(value: cc),
                                subject: subject, bodyMarkdown: body, replyTo: replyTo, attachments: attachments)
    }
}

@MainActor @Observable
final class MailModel {
    var core: LagomCore?
    var accounts: [MailAccount] = []
    var rows: [MailRow] = []
    var failures: [SendFailure] = []
    var accountID: String?
    var mailbox: MailboxChoice = .inbox
    var searchText = ""
    var remoteImagesID: Int64?
    var inlineRevision = 0
    private var selectionRevision = 0
    var selectedID: Int64?
    var message: MailMessage?
    var error: String?
    var notice: String?
    var composeDraft: ComposeDraft?
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

    deinit { wakeTask?.cancel() }

    func start() async {
        guard core == nil else { return }
        do {
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
            core = try await Task.detached { try LagomCore.open(dataDir: directory.path) }.value
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
                let batch = try await query.isEmpty
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
        if active {
            await reload()
            await sync()
        }
    }

    func changeScope() async {
        selectionRevision += 1
        remoteImagesID = nil
        hasMore = false
        selectedID = nil
        message = nil
        rows = []
        await reload()
    }

    func select(_ id: Int64?) async {
        selectionRevision += 1
        let request = selectionRevision
        remoteImagesID = nil
        inlineRevision = 0
        message = nil
        guard let core, let id else { return }
        do {
            let loaded = try await core.message(id: id)
            guard selectedID == id, request == selectionRevision else { return }
            message = loaded
            async let inline: Void = loadInline(id: id, request: request)
            if loaded.row.unread { await setRead(id: id, read: true) }
            await inline
        } catch { if selectedID == id, request == selectionRevision { self.error = error.localizedDescription } }
    }

    private func loadInline(id: Int64, request: Int) async {
        guard let core, message?.htmlBody != nil else { return }
        do {
            try await core.loadInline(id: id)
            guard selectedID == id, request == selectionRevision else { return }
            inlineRevision += 1
        } catch { if selectedID == id, request == selectionRevision { self.error = error.localizedDescription } }
    }

    func loadRemoteImages(id: Int64) {
        guard selectedID == id, message?.row.id == id else { return }
        remoteImagesID = id
    }

    func setRead(id: Int64, read: Bool) async {
        guard let core, actingID != id, !readingIDs.contains(id) else { return }
        readingIDs.insert(id)
        defer { readingIDs.remove(id) }
        do {
            try await core.markRead(id: id, read: read)
            if selectedID == id {
                let request = selectionRevision
                let updated = try await core.message(id: id)
                if selectedID == id, request == selectionRevision { message = updated }
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

    func act(_ action: MailAction, id: Int64) async {
        guard let core, !working else { return }
        actingID = id
        defer { actingID = nil }
        do {
            switch action {
            case .done: try await core.done(ids: [id])
            case .later: try await core.later(ids: [id], untilUnix: tomorrowAtEight())
            case .delete: try await core.delete(ids: [id])
            }
            if selectedID == id { selectedID = nil; message = nil }
        } catch { self.error = error.localizedDescription }
        await reload()
    }

    func prepareCompose(id: Int64, kind: ComposeKind) async {
        guard let core, !preparingCompose, composeDraft == nil else { return }
        preparingCompose = true
        defer { preparingCompose = false }
        do {
            let reply = try await core.composeDraft(id: id, kind: kind)
            guard selectedID == id, composeDraft == nil else { return }
            composeDraft = ComposeDraft(reply: reply, kind: kind)
        } catch { if selectedID == id { self.error = error.localizedDescription } }
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
