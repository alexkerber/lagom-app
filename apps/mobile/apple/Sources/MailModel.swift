import Foundation
import Observation

enum MailboxChoice: String, CaseIterable, Identifiable {
    case inbox = "Inbox", later = "Later", done = "Done", sent = "Sent"
    var id: String { rawValue }
    var mailbox: Mailbox {
        switch self {
        case .inbox: .inbox
        case .later: .later
        case .done: .done
        case .sent: .sent
        }
    }
    var symbol: String {
        switch self {
        case .inbox: "tray"
        case .later: "clock"
        case .done: "checkmark"
        case .sent: "paperplane"
        }
    }
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
    var selectedID: Int64?
    var message: MailMessage?
    var error: String?
    var notice: String?
    var composeDraft: ComposeDraft?
    var preparingCompose = false
    var syncing = false
    var working = false
    var hasMore = false
    private var revision = 0
    private var syncAgain = false
    private var foreground = true
    @ObservationIgnored private var wakeTask: Task<Void, Never>?

    deinit { wakeTask?.cancel() }

    func start() async {
        guard core == nil else { return }
        do {
            let directory = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
                .appendingPathComponent("Lagom", isDirectory: true)
            core = try await Task.detached { try LagomCore.open(dataDir: directory.path) }.value
            await reload()
            await sync()
        } catch { self.error = error.localizedDescription }
    }

    func reload(more: Bool = false) async {
        guard let core else { return }
        revision += 1
        let request = revision
        let offset = more ? UInt32(rows.count) : 0
        let account = accountID
        let mailbox = mailbox.mailbox
        do {
            let due = try await core.nextSnooze()
            let accounts = try await core.accounts()
            var page: [MailRow] = []
            let count = more ? 100 : max(100, rows.count)
            var hasMore = false
            repeat {
                let batch = try await core.mailbox(accountId: account, mailbox: mailbox, limit: 100, offset: offset + UInt32(page.count))
                page += batch
                hasMore = batch.count == 100
            } while hasMore && page.count < count
            let failures = try await core.sendFailures()
            guard request == revision else { return }
            self.accounts = accounts
            self.failures = failures
            rows = more ? rows + page.filter { next in !rows.contains { $0.id == next.id } } : page
            self.hasMore = hasMore
            wake(at: due)
        } catch { if request == revision { self.error = error.localizedDescription } }
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
        selectedID = nil
        message = nil
        rows = []
        await reload()
    }

    func select(_ id: Int64?) async {
        message = nil
        guard let core, let id else { return }
        do {
            let loaded = try await core.message(id: id)
            guard selectedID == id else { return }
            message = loaded
            if loaded.row.unread {
                try await core.markRead(id: id, read: true)
                guard selectedID == id else { return }
                let read = try await core.message(id: id)
                guard selectedID == id else { return }
                message = read
                await reload()
            }
        } catch { if selectedID == id { self.error = error.localizedDescription } }
    }

    func sync() async {
        guard let core else { return }
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
        working = true
        defer { working = false }
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
