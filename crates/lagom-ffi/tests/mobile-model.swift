import Foundation

@MainActor final class FixtureCore: LagomCore, @unchecked Sendable {
    override func cancelBackgroundSync() {}
    override func backgroundSync() async throws -> [MailRow] { [] }
    override func refreshContacts(accountId: String) async throws {}
    override func suggestContacts(accountId: String, query: String) -> [ContactSuggestion] { [] }
    var attachmentCalls = 0
    var attachmentGate: CheckedContinuation<Void, Never>?
    var suspendAttachment = false
    var failAttachment = false
    override func attachment(messageId: Int64, section: String) async throws -> Data {
        precondition(messageId == 42 && section == "2")
        attachmentCalls += 1
        if suspendAttachment { await withCheckedContinuation { attachmentGate = $0 } }
        if failAttachment { throw MailError.Failed(reason: "download failed") }
        return Data([0, 255])
    }
    var deadline: Int64?
    var reads = 0
    var syncs = 0
    var suspendDone = false
    var doneContinuation: CheckedContinuation<Void, Error>?
    var flags: [Int64] = []
    var mail = (1...650).map { id in
        MailRow(id: Int64(id), account: "fixture", senderName: "Sender", senderAddress: "sender@example.test", subject: "Mail", preview: "", dateUnix: Int64(id), unread: true, hasAttachments: false, threadId: "thread-\(Int64(id))", messageCount: 1, messageIds: [Int64(id)])
    }
    override func accounts() async throws -> [MailAccount] { [] }
    override func sendFailures() async throws -> [SendFailure] { [] }
    override func localDrafts() async throws -> [DraftRow] { [] }
    override func mailbox(accountId: String?, mailbox: Mailbox, limit: UInt32, offset: UInt32) async throws -> [MailRow] {
        reads += 1
        precondition(limit <= 500)
        guard mailbox == .inbox else { return [] }
        let visible = mail.filter { $0.id != 1 || deadline == nil || Double(deadline!) <= Date().timeIntervalSince1970 }
        return Array(visible.dropFirst(Int(offset)).prefix(Int(limit)))
    }
    var conversationIDs: [Int64] = []
    override func conversation(messageId: Int64) async throws -> [MailMessage] {
        var result: [MailMessage] = []
        for id in conversationIDs.isEmpty ? [messageId] : conversationIDs { result.append(try await message(id: id)) }
        return result
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
    override func cancelBackgroundSync() {}
    override func backgroundSync() async throws -> [MailRow] { [] }
    override func refreshContacts(accountId: String) async throws {}
    override func suggestContacts(accountId: String, query: String) -> [ContactSuggestion] { [] }
    var drafts: [String: LocalDraft] = [:]
    var saves = 0
    var gate: CheckedContinuation<Void, Never>?
    var suspendSave = false
    var fail = false
    var sent: LocalDraft?
    var failDraftList = false
    var suspendDraftList = false
    var draftListGate: CheckedContinuation<Void, Never>?
    override func accounts() async throws -> [MailAccount] { [] }
    override func nextSnooze() async throws -> Int64? { nil }
    override func sendFailures() async throws -> [SendFailure] { [] }
    override func mailbox(accountId: String?, mailbox: Mailbox, limit: UInt32, offset: UInt32) async throws -> [MailRow] {
        [MailRow(id: 1, account: "fixture", senderName: "Sender", senderAddress: "s@example.test", subject: "Inbox", preview: "", dateUnix: 0, unread: false, hasAttachments: false, threadId: "thread-\(1)", messageCount: 1, messageIds: [1])]
    }
    override func localDrafts() async throws -> [DraftRow] {
        let rows = drafts.values.map(\.row)
        if suspendDraftList { await withCheckedContinuation { draftListGate = $0 } }
        if failDraftList { throw MailError.Failed(reason: "draft list failed") }
        return rows
    }
    override func localDraft(id: String) async throws -> LocalDraft {
        guard let draft = drafts[id] else { throw MailError.NotFound }
        return draft
    }
    override func savedReply(id: Int64) async throws -> LocalDraft? { drafts.values.first }
    override func saveDraft(id: String, accountId: String, to: String, cc: String, subject: String, body: String,
                            replyTo: Int64?, attachments: [SendAttachment]) async throws -> DraftRow {
        saves += 1
        if suspendSave { await withCheckedContinuation { gate = $0 } }
        if fail { throw MailError.Failed(reason: "save refused") }
        let saved = LocalDraft(id: id, account: accountId, to: to, cc: cc, subject: subject, body: body,
                              inReplyTo: nil, references: [], attachments: attachments)
        drafts[id] = saved
        return saved.row
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
    @MainActor static func listReadActionIncludesOlderUnreadMembers() async {
        let core = FixtureCore(noHandle: .init())
        core.mail = Array(core.mail.prefix(3))
        core.mail[1].unread = false
        let model = MailModel(); model.core = core
        var row = core.mail[1]
        row.messageIds = [2, 1]; row.messageCount = 2; row.unread = true
        await model.setRead(row: row)
        precondition(core.flags == [2, 1] && core.mail.map(\.unread) == [false, false, true])
        row.unread = false
        await model.setRead(row: row)
        precondition(core.flags == [2, 1, 2, 1] && core.mail.map(\.unread) == [true, true, true])
    }

    @MainActor static func selectedThreadActionSurvivesRepresentativeChange() async {
        let core = FixtureCore(noHandle: .init())
        core.mail = Array(core.mail.prefix(3))
        core.mail[0].unread = false
        let model = MailModel(); model.core = core; model.selectedID = 1
        await model.select(1)
        core.mail[1].messageIds = [2, 1]; core.mail[1].messageCount = 2
        core.mail.removeFirst()
        await model.sync()
        precondition(model.selectedID == 1 && model.rows.map(\.id) == [2, 3])
        await model.act(.done, id: model.selectedID!)
        precondition(core.mail.map(\.id) == [3] && model.selectedID == nil)
    }

    @MainActor static func explicitlyOpenedReadMemberStartsExpanded() async {
        let core = FixtureCore(noHandle: .init())
        core.conversationIDs = [1, 2, 3, 4]
        for index in [0, 1, 3] { core.mail[index].unread = false }
        let model = MailModel(); model.core = core; model.selectedID = 1
        await model.select(1)
        precondition(model.message?.row.id == 1 && model.expandedIDs == [1, 3, 4])
        precondition(core.flags == [3])
    }

    @MainActor static func attachments() async throws {
        for (name, safe) in [("../../file.txt", "file.txt"), ("..\\file.txt", "file.txt"),
                             (" \0\n", "attachment"), ("", "attachment"), ("..", "attachment"),
                             (" a\u{7f}b ", "ab")] { precondition(safeAttachmentName(name) == safe) }
        precondition(safeAttachmentName(String(repeating: "x", count: 121)).count == 120)
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let model = MailModel(attachmentDirectory: root.appendingPathComponent("attachments"))
        let core = FixtureCore(noHandle: .init()); model.core = core
        let part = MailAttachment(section: "2", filename: "../fixture.bin", mimeType: "", size: 2)
        core.suspendAttachment = true
        let pending = Task { await model.openAttachment(messageID: 42, part: part) }
        while core.attachmentGate == nil { await Task.yield() }
        await model.openAttachment(messageID: 42, part: part)
        precondition(core.attachmentCalls == 1 && model.downloadingAttachments == ["42:2"])
        core.attachmentGate?.resume(); await pending.value
        let url = model.attachmentURL!
        let bytes = try Data(contentsOf: url)
        precondition(bytes == Data([0, 255]) && url.lastPathComponent == "fixture.bin")
        precondition(UUID(uuidString: url.deletingLastPathComponent().lastPathComponent) != nil)
        precondition(url.deletingLastPathComponent().deletingLastPathComponent().path == model.attachmentDirectory.path)
        precondition(model.downloadingAttachments.isEmpty)
        model.attachmentURL = nil
        core.suspendAttachment = false; core.failAttachment = true
        await model.openAttachment(messageID: 42, part: part)
        precondition(model.attachmentURL == nil && model.error != nil && model.downloadingAttachments.isEmpty)
        model.error = nil; core.failAttachment = false; core.suspendAttachment = true; core.attachmentGate = nil
        let stale = Task { await model.openAttachment(messageID: 42, part: part) }
        while core.attachmentGate == nil { await Task.yield() }
        await model.select(nil)
        core.attachmentGate?.resume(); await stale.value
        precondition(model.attachmentURL == nil && model.downloadingAttachments.isEmpty)
        print("Attachment safe names, bytes, double tap, error and stale selection checks passed")
    }

    @MainActor static func main() async throws {
        await listReadActionIncludesOlderUnreadMembers()
        await selectedThreadActionSurvivesRepresentativeChange()
        await explicitlyOpenedReadMemberStartsExpanded()
        print("Conversation row read, changed representative and explicit expansion regressions passed")
        let conversationCore = FixtureCore(noHandle: .init())
        conversationCore.conversationIDs = [1, 2, 3]
        conversationCore.mail[0].unread = false
        conversationCore.mail[2].unread = false
        let conversationModel = MailModel(); conversationModel.core = conversationCore; conversationModel.selectedID = 3
        await conversationModel.select(3)
        precondition(conversationModel.messages.map { $0.row.id } == [1, 2, 3])
        precondition(conversationModel.expandedIDs == [2, 3] && conversationCore.flags == [2])
        conversationModel.loadRemoteImages(id: 2)
        await conversationModel.toggleMessage(2)
        await conversationModel.setRead(id: 2, read: false)
        await conversationModel.toggleMessage(2)
        precondition(!conversationModel.messages[1].row.unread && conversationCore.flags == [2, 2, 2])
        await conversationModel.toggleMessage(1)
        precondition(conversationModel.expandedIDs == [1, 2, 3] && conversationModel.remoteImagesIDs == [2])
        conversationModel.rows = [conversationCore.mail[2]]
        conversationModel.rows[0].messageCount = 2
        conversationModel.rows[0].messageIds = [3, 1]
        await conversationModel.act(.done, id: 3)
        precondition(!conversationCore.mail.contains { $0.id == 1 || $0.id == 3 })
        await conversationModel.changeScope()
        precondition(conversationModel.messages.isEmpty && conversationModel.expandedIDs.isEmpty && conversationModel.remoteImagesIDs.isEmpty)

        let first = MailRow(id: 1, account: "a", senderName: "", senderAddress: "sender@example.test", subject: "First", preview: "", dateUnix: 1, unread: false, hasAttachments: false, threadId: "thread-\(1)", messageCount: 1, messageIds: [1])
        precondition(mailNotices([]).isEmpty)
        precondition(mailNotices([first]) == [MailNotice(target: MailTarget(account: "a", message: 1), title: "sender@example.test", body: "First")])
        var second = first; second.id = 2; second.dateUnix = 2; second.subject = "Second"
        var named = first; named.account = "b"; named.senderName = "Sender"
        let notices = mailNotices([first, named, second])
        precondition(notices.count == 2 && notices[0] == MailNotice(target: MailTarget(account: "a", message: 2), title: "2 new messages", body: "Second\nFirst"))
        precondition(notices[1].title == "Sender")
        let notificationModel = MailModel()
        notificationModel.mailbox = .sent; notificationModel.searchText = "old"
        notificationModel.openNotification(MailTarget(account: "fixture", message: 42))
        await notificationModel.changeScope()
        precondition(notificationModel.notificationTarget != nil)
        notificationModel.core = FixtureCore(noHandle: .init())
        await notificationModel.changeScope()
        precondition(notificationModel.accountID == "fixture" && notificationModel.mailbox == .inbox && notificationModel.searchText.isEmpty)
        precondition(notificationModel.selectedID == 42 && notificationModel.notificationTarget == nil)
        await notificationModel.select(notificationModel.selectedID)
        precondition(notificationModel.message?.row.id == 42)

        precondition(recipientToken("") == "")
        precondition(recipientToken("  ") == "")
        precondition(recipientToken("An") == "An")
        precondition(recipientToken("one@example.com,  An ") == "An")
        precondition(recipientToken("one@example.com;\r\nAn") == "An")
        precondition(recipientToken("one@example.com, ") == "")
        precondition(recipientToken("\"Team, West\" <west@example.com>, An") == "An")
        precondition(recipientToken("\"Team, West") == "\"Team, West")
        precondition(recipientToken("\"Team\\\", West\" <west@example.com>; An") == "An")
        precondition(recipientToken("One (team, (west; east)) <one@example.com>, An") == "An")
        precondition(recipientToken("odd <one,two") == "odd <one,two")
        precondition(recipientToken("Åsa@example.com, Éq") == "Éq")
        precondition(recipientToken("An", replacingWith: "\"Team, West\" <west@example.com>") == "\"Team, West\" <west@example.com>, ")
        precondition(recipientToken("one@example.com,  An", replacingWith: "\"Team, West\" <west@example.com>") == "one@example.com,  \"Team, West\" <west@example.com>, ")
        precondition(recipientToken("\"Team, West\" <west@example.com>, We", replacingWith: "\"Team, West\" <west@example.com>") == "\"Team, West\" <west@example.com>, \"Team, West\" <west@example.com>, ")
        precondition(recipientToken("one@example.com;\r\nWe", replacingWith: "\"Team, West\" <west@example.com>") == "one@example.com;\r\n\"Team, West\" <west@example.com>, ")
        precondition(recipientToken("\"Team, W", replacingWith: "\"Team, West\" <west@example.com>") == "\"Team, West\" <west@example.com>, ")
        print("Recipient token checks passed")
        if CommandLine.arguments.contains("--wake") {
            await wake()
            return
        }
        try await attachments()
        try await draftReload()
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

    @MainActor static func draftReload() async throws {
        for (email, domain) in [("name@earlier@example.test", "example.test"), ("missing", "localhost")] {
            let id = draftMessageID(email: email)
            precondition(id.hasSuffix("@\(domain)>"))
            precondition(id.dropFirst().prefix(32).allSatisfy { "0123456789abcdef".contains($0) })
            precondition(id.count == 35 + domain.count)
        }
        let core = DraftFixtureCore(noHandle: .init())
        let model = MailModel(); model.core = core
        core.failDraftList = true
        await model.reload()
        precondition(model.rows.map(\.id) == [1] && model.error != nil)
        core.failDraftList = false
        model.composeDraft = ComposeDraft(account: "fixture")
        model.composeDraft?.body = "saved"
        try await model.closeCompose(discard: false)
        await model.openDraft(model.drafts[0])
        core.suspendDraftList = true
        let load = Task { await model.reload() }
        while core.draftListGate == nil { await Task.yield() }
        try await model.closeCompose(discard: true)
        core.draftListGate?.resume()
        await load.value
        precondition(model.drafts.isEmpty && model.savedDraftIDs.isEmpty)
        precondition(model.rows.map(\.id) == [1])
        core.suspendDraftList = false
        model.composeDraft = ComposeDraft(account: "fixture")
        model.composeDraft?.body = "deleted elsewhere"
        try await model.closeCompose(discard: false)
        let row = model.drafts[0]
        core.drafts = [:]
        await model.openDraft(row)
        precondition(model.drafts.isEmpty && model.composeDraft == nil)
        print("MailModel: Message-ID, isolated draft reload, discard race and missing draft passed")
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
        // CI runners oversleep; poll instead of racing the 0.5 s debounce.
        for _ in 0..<50 where core.drafts.values.first?.body != "latest" { try await Task.sleep(for: .milliseconds(100)) }
        precondition(core.drafts.values.first?.body == "latest")
        let sheetID = model.composeDraft!.id
        let old = model.composeDraft!.messageID
        try await model.changeComposeAccount("second")
        precondition(model.composeDraft!.messageID != old && core.drafts.count == 1)
        precondition(model.composeDraft!.id == sheetID, "account change replaced sheet identity")
        precondition(core.drafts.values.first?.account == "second" && core.drafts.values.first?.body == "latest")
        model.composeDraft?.body = ""
        // CI runners oversleep; poll instead of racing the 0.5 s debounce.
        for _ in 0..<50 where core.drafts.values.first?.body != "" { try await Task.sleep(for: .milliseconds(100)) }
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
        let stale = core.drafts.values.first!.row
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

private extension LocalDraft {
    var row: DraftRow { DraftRow(id: id, account: account, to: to, subject: subject, updatedUnix: 42) }
}
