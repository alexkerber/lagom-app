import Foundation

@MainActor final class FixtureCore: LagomCore, @unchecked Sendable {
    var deadline: Int64?
    var reads = 0
    var syncs = 0
    var suspendDone = false
    var doneContinuation: CheckedContinuation<Void, Error>?
    var flags: [Int64] = []
    var mail = (1...650).map { id in
        MailRow(id: Int64(id), account: "fixture", senderName: "Sender", senderAddress: "sender@example.test", subject: "Mail", preview: "", dateUnix: Int64(id), unread: true, hasAttachments: false)
    }
    override func accounts() async throws -> [MailAccount] { [] }
    override func sendFailures() async throws -> [SendFailure] { [] }
    override func localDrafts() async throws -> [LocalDraft] { [] }
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
        flags.append(id)
        let index = mail.firstIndex { $0.id == id }!
        mail[index].unread = !read
    }
    override func done(ids: [Int64]) async throws {
        if suspendDone { try await withCheckedThrowingContinuation { doneContinuation = $0 } }
        mail.removeAll { ids.contains($0.id) }
    }
    override func nextSnooze() async throws -> Int64? {
        guard let deadline, Double(deadline) > Date().timeIntervalSince1970 else { return nil }
        return deadline
    }
    override func syncAll() async throws { syncs += 1 }
}

@MainActor final class DraftFixtureCore: LagomCore, @unchecked Sendable {
    var drafts: [String: LocalDraft] = [:]
    var saves = 0
    var gate: CheckedContinuation<Void, Never>?
    var suspendSave = false
    var fail = false
    var sent: LocalDraft?
    override func localDrafts() async throws -> [LocalDraft] { Array(drafts.values) }
    override func savedReply(id: Int64) async throws -> LocalDraft? { drafts.values.first }
    override func saveDraft(id: String, accountId: String, to: String, cc: String, subject: String, body: String,
                            replyTo: Int64?, attachments: [SendAttachment]) async throws -> LocalDraft {
        saves += 1
        if suspendSave { await withCheckedContinuation { gate = $0 } }
        if fail { throw MailError.Failed(reason: "save refused") }
        let saved = LocalDraft(id: id, account: accountId, to: to, cc: cc, subject: subject, body: body,
                              inReplyTo: nil, references: [], attachments: attachments, updatedUnix: 42)
        drafts[id] = saved
        return saved
    }
    override func deleteDraft(id: String) async throws {
        if fail { throw MailError.Failed(reason: "discard refused") }
        drafts[id] = nil
    }
    override func sendDraft(id: String) async throws -> String {
        if fail { throw MailError.Failed(reason: "enqueue refused") }
        sent = drafts.removeValue(forKey: id)
        return id
    }
}

@main
struct ModelChecks {
    @MainActor static func main() async throws {
        if CommandLine.arguments.contains("--wake") {
            await wake()
            return
        }
        try await draftLifecycle()
        await openingDuringArchive()
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

    @MainActor static func draftLifecycle() async throws {
        let core = DraftFixtureCore(noHandle: .init())
        let model = MailModel(); model.core = core
        model.composeDraft = ComposeDraft(account: "fixture")
        model.composeDraft?.body = "first"
        try await Task.sleep(for: .milliseconds(300))
        model.composeDraft?.body = "latest"
        try await Task.sleep(for: .milliseconds(300))
        precondition(core.saves == 0, "edits must debounce")
        try await Task.sleep(for: .milliseconds(300))
        precondition(core.drafts.values.first?.body == "latest")
        let old = model.composeDraft!.id
        try await model.changeComposeAccount("second")
        precondition(model.composeDraft!.id != old && core.drafts.count == 1)
        precondition(core.drafts.values.first?.account == "second" && core.drafts.values.first?.body == "latest")
        model.composeDraft?.body = ""
        try await Task.sleep(for: .milliseconds(600))
        precondition(core.drafts.values.first?.body == "", "clearing text must autosave")
        model.composeDraft?.body = "latest"
        model.composeDraft?.subject = "Last edit"
        try await model.closeCompose(discard: false)
        precondition(model.composeDraft == nil && core.drafts.values.first?.subject == "Last edit")
        model.selectedID = 42
        await model.prepareCompose(id: 42, kind: .reply)
        precondition(model.composeDraft?.body == "latest" && model.composeDraft?.restored == true)
        model.composeDraft?.body = "keep after error"
        core.fail = true
        do { try await model.closeCompose(discard: false); preconditionFailure("save should fail") } catch { }
        do { try await model.closeCompose(discard: true); preconditionFailure("discard should fail") } catch { }
        precondition(model.composeDraft?.body == "keep after error")
        core.fail = false
        try await model.sendCompose()
        precondition(model.composeDraft == nil && core.drafts.isEmpty && core.sent?.body == "keep after error")
        model.composeDraft = ComposeDraft(account: "fixture")
        model.composeDraft?.body = "dismissed"
        model.composeDraft = nil
        try await Task.sleep(for: .milliseconds(100))
        precondition(core.drafts.values.first?.body == "dismissed", "dismissal must flush")
        let stale = core.drafts.values.first!
        core.drafts[stale.id]?.body = "latest on disk"
        await model.openDraft(stale)
        precondition(model.composeDraft?.body == "latest on disk", "opening must not restore an old list snapshot")
        core.suspendSave = true
        model.composeDraft?.body = "discard me"
        while core.gate == nil { await Task.yield() }
        let close = Task { try await model.closeCompose(discard: true) }
        await Task.yield()
        precondition(model.composeDraft != nil)
        core.gate?.resume()
        try await close.value
        try await Task.sleep(for: .milliseconds(600))
        precondition(model.composeDraft == nil && core.drafts.isEmpty, "a delayed save resurrected a discard")
        print("MailModel: durable draft lifecycle passed")
    }

    @MainActor static func openingDuringArchive() async {
        for rejected in [false, true] {
            let core = FixtureCore(noHandle: .init())
            core.suspendDone = true
            let model = MailModel()
            model.core = core
            let pending = Task { await model.act(.done, id: 1) }
            while core.doneContinuation == nil { await Task.yield() }
            await model.setRead(id: 1, read: true)
            precondition(core.flags.isEmpty, "the archived message must remain guarded")
            model.selectedID = 2
            await model.select(2)
            precondition(core.flags == [2], "opening another message dropped its read update")
            precondition(model.message?.row.unread == false && model.rows[1].unread == false)
            precondition(core.mail[1].unread == false && model.working)
            if rejected { core.doneContinuation?.resume(throwing: MailError.Failed(reason: "archive refused")) }
            else { core.doneContinuation?.resume() }
            await pending.value
            precondition(!model.working && model.selectedID == 2 && model.message?.row.id == 2)
            precondition(model.message?.row.unread == false && model.rows.first { $0.id == 2 }?.unread == false)
            precondition((model.error != nil) == rejected)
        }
        print("MailModel: opening during archive regression passed (success and failure)")
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
