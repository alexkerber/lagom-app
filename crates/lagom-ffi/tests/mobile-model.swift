import Foundation

final class FixtureCore: LagomCore, @unchecked Sendable {
    var deadline: Int64?
    var reads = 0
    var syncs = 0
    var mail = (1...650).map { id in
        MailRow(id: Int64(id), account: "fixture", senderName: "Sender", senderAddress: "sender@example.test", subject: "Mail", preview: "", dateUnix: Int64(id), unread: true, hasAttachments: false)
    }
    override func accounts() async throws -> [MailAccount] { [] }
    override func sendFailures() async throws -> [SendFailure] { [] }
    override func mailbox(accountId: String?, mailbox: Mailbox, limit: UInt32, offset: UInt32) async throws -> [MailRow] {
        reads += 1
        precondition(limit <= 500)
        guard mailbox == .inbox else { return [] }
        let visible = mail.filter { $0.id != 1 || deadline == nil || Double(deadline!) <= Date().timeIntervalSince1970 }
        return Array(visible.dropFirst(Int(offset)).prefix(Int(limit)))
    }
    override func message(id: Int64) async throws -> MailMessage {
        MailMessage(row: mail.first { $0.id == id }!, textBody: "body", htmlBody: nil, attachments: [])
    }
    override func markRead(id: Int64, read: Bool) async throws {
        let index = mail.firstIndex { $0.id == id }!
        mail[index].unread = !read
    }
    override func done(ids: [Int64]) async throws { mail.removeAll { ids.contains($0.id) } }
    override func nextSnooze() async throws -> Int64? {
        guard let deadline, Double(deadline) > Date().timeIntervalSince1970 else { return nil }
        return deadline
    }
    override func syncAll() async throws { syncs += 1 }
}

@main
struct ModelChecks {
    @MainActor static func main() async throws {
        if CommandLine.arguments.contains("--wake") {
            await wake()
            return
        }
        let core = FixtureCore(noHandle: .init())
        let model = MailModel()
        model.core = core
        await model.reload()
        await model.reload(more: true)
        precondition(model.rows.count == 200)
        model.selectedID = 150
        await model.select(150)
        precondition(model.rows.count == 200, "mark-read discarded the second page")
        precondition(model.rows.contains { $0.id == 150 && !$0.unread })
        precondition(model.selectedID == 150 && model.message?.row.id == 150)
        for _ in 0..<4 { await model.reload(more: true) }
        precondition(model.rows.count == 600)
        await model.sync()
        precondition(model.rows.count == 600 && model.hasMore, "sync discarded loaded pages")
        await model.act(.done, id: 1)
        precondition(model.rows.count == 600 && model.rows.first?.id == 2, "action discarded loaded pages")
        await model.reload(more: true)
        precondition(model.rows.count == 649 && !model.hasMore)
        await model.reload()
        precondition(model.rows.count == 649 && !model.hasMore)
        await model.changeScope()
        precondition(model.rows.count == 100 && model.selectedID == nil)
        model.mailbox = .done
        await model.changeScope()
        precondition(model.rows.isEmpty && !model.hasMore)
        print("MailModel: pagination regression passed")
    }

    @MainActor static func wake() async {
        let core = FixtureCore(noHandle: .init())
        let model = MailModel()
        model.core = core
        core.deadline = Int64(Date().timeIntervalSince1970) + 2
        await model.reload()
        precondition(!model.rows.contains { $0.id == 1 })
        try? await Task.sleep(for: .seconds(Double(core.deadline!) - Date().timeIntervalSince1970 + 0.2))
        precondition(model.rows.contains { $0.id == 1 }, "foreground Inbox did not wake snoozed mail")
        precondition(core.syncs == 0, "deadline wake must stay local")
        core.deadline = Int64(Date().timeIntervalSince1970) + 2
        await model.reload()
        precondition(!model.rows.contains { $0.id == 1 })
        await model.setForeground(false)
        let reads = core.reads
        try? await Task.sleep(for: .seconds(Double(core.deadline!) - Date().timeIntervalSince1970 + 0.2))
        precondition(core.reads == reads, "backgrounding must cancel the wake")
        await model.setForeground(true)
        precondition(model.rows.contains { $0.id == 1 }, "resume must reload expired mail")
        core.deadline = Int64(Date().timeIntervalSince1970) + 2
        await model.reload()
        core.deadline = nil
        await model.reload()
        let afterCancel = core.reads
        try? await Task.sleep(for: .seconds(2.2))
        precondition(core.reads == afterCancel, "removing a snooze must cancel its wake")
        print("MailModel: foreground wake regression passed")
    }
}
