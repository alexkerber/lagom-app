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
    var syncing = false
    var working = false
    var hasMore = false
    private var revision = 0
    private var syncAgain = false

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
            let accounts = try await core.accounts()
            let page = try await core.mailbox(accountId: account, mailbox: mailbox, limit: 100, offset: offset)
            let failures = try await core.sendFailures()
            guard request == revision else { return }
            self.accounts = accounts
            self.failures = failures
            rows = more ? rows + page.filter { next in !rows.contains { $0.id == next.id } } : page
            hasMore = page.count == 100
        } catch { if request == revision { self.error = error.localizedDescription } }
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
