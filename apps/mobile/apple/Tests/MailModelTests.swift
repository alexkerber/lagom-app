import XCTest

final class MailModelTests: XCTestCase {
    func testNotificationContentGroupsAccountsAndTargetsNewestMail() {
        let row = MailRow(id: 1, account: "a", senderName: "", senderAddress: "sender@example.test", subject: "First", preview: "", dateUnix: 1, unread: true, hasAttachments: false, threadId: "thread-\(1)", messageCount: 1, messageIds: [1])
        XCTAssertTrue(mailNotices([]).isEmpty)
        XCTAssertEqual(mailNotices([row]), [MailNotice(target: MailTarget(account: "a", message: 1), title: "sender@example.test", body: "First")])
        var named = row; named.senderName = "Sender"; named.account = "b"
        XCTAssertEqual(mailNotices([named]).first?.title, "Sender")
        var second = row; second.id = 2; second.dateUnix = 2; second.subject = "Second"
        var third = second; third.id = 3; third.subject = "Third"
        var fourth = third; fourth.id = 4; fourth.subject = "Fourth"
        let notices = mailNotices([row, named, second, fourth, third])
        XCTAssertEqual(notices.count, 2)
        XCTAssertEqual(notices[0], MailNotice(target: MailTarget(account: "a", message: 4), title: "4 new messages", body: "Fourth\nThird\nSecond"))
    }

    @MainActor func testNotificationWaitsForStoreThenSelectsAccountAndMessage() async {
        let model = MailModel()
        model.mailbox = .sent; model.searchText = "old search"
        let target = MailTarget(account: "second", message: 1)
        model.openNotification(target)
        await model.changeScope()
        XCTAssertEqual(model.notificationTarget, target)
        model.core = DailyFixtureCore(noHandle: .init())
        await model.changeScope()
        XCTAssertEqual(model.accountID, "second")
        XCTAssertEqual(model.mailbox, .inbox)
        XCTAssertEqual(model.searchText, "")
        XCTAssertEqual(model.selectedID, 1)
        XCTAssertNil(model.notificationTarget)
        await model.select(model.selectedID)
        XCTAssertEqual(model.message?.row.id, 1)
    }

    func testRecipientTokenUsesDesktopBoundariesAndPreservesEarlierAddresses() {
        XCTAssertEqual(recipientToken(""), "")
        XCTAssertEqual(recipientToken("  "), "")
        XCTAssertEqual(recipientToken("An"), "An")
        XCTAssertEqual(recipientToken("one@example.com,  An "), "An")
        XCTAssertEqual(recipientToken("one@example.com;\r\nAn"), "An")
        XCTAssertEqual(recipientToken("one@example.com, "), "")
        XCTAssertEqual(recipientToken("\"Team, West\" <west@example.com>, An"), "An")
        XCTAssertEqual(recipientToken("\"Team, West"), "\"Team, West")
        XCTAssertEqual(recipientToken("\"Team\\\", West\" <west@example.com>; An"), "An")
        XCTAssertEqual(recipientToken("One (team, (west; east)) <one@example.com>, An"), "An")
        XCTAssertEqual(recipientToken("odd <one,two"), "odd <one,two")
        XCTAssertEqual(recipientToken("Åsa@example.com, Éq"), "Éq")
        XCTAssertEqual(recipientToken("An", replacingWith: "\"Team, West\" <west@example.com>"), "\"Team, West\" <west@example.com>, ")
        XCTAssertEqual(recipientToken("one@example.com,  An", replacingWith: "\"Team, West\" <west@example.com>"), "one@example.com,  \"Team, West\" <west@example.com>, ")
        XCTAssertEqual(recipientToken("\"Team, West\" <west@example.com>, We", replacingWith: "\"Team, West\" <west@example.com>"), "\"Team, West\" <west@example.com>, \"Team, West\" <west@example.com>, ")
        XCTAssertEqual(recipientToken("one@example.com;\r\nWe", replacingWith: "\"Team, West\" <west@example.com>"), "one@example.com;\r\n\"Team, West\" <west@example.com>, ")
        XCTAssertEqual(recipientToken("\"Team, W", replacingWith: "\"Team, West\" <west@example.com>"), "\"Team, West\" <west@example.com>, ")
    }

    func testSafeAttachmentName() {
        for (input, expected) in [("../../secret.txt", "secret.txt"), ("..\\..\\safe.pdf", "safe.pdf"),
                                  ("", "attachment"), (" . ", "attachment"), ("..", "attachment"),
                                  ("dir/", "attachment"), (" \0\n\t ", "attachment"),
                                  (" a\0b\u{7f}c.txt ", "abc.txt")] {
            XCTAssertEqual(safeAttachmentName(input), expected)
        }
        XCTAssertEqual(safeAttachmentName(String(repeating: "x", count: 121)).count, 120)
    }

    @MainActor func testAttachmentWritesBytesBlocksDoubleTapsAndReportsFailure() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let core = ComposeFixtureCore(noHandle: .init())
        let model = MailModel(attachmentDirectory: directory.appendingPathComponent("attachments"))
        model.core = core
        let part = MailAttachment(section: "2", filename: "../../fixture.bin", mimeType: "application/octet-stream", size: 2)
        core.suspendAttachment = true
        let pending = Task { await model.openAttachment(messageID: 42, part: part) }
        while core.attachmentGate == nil { await Task.yield() }
        XCTAssertTrue(model.downloadingAttachments.contains("42:2"))
        await model.openAttachment(messageID: 42, part: part)
        XCTAssertEqual(core.attachmentCalls.count, 1)
        XCTAssertEqual(core.attachmentCalls.first?.0, 42)
        XCTAssertEqual(core.attachmentCalls.first?.1, "2")
        core.attachmentGate?.resume(); await pending.value
        let url = try XCTUnwrap(model.attachmentURL)
        XCTAssertEqual(try Data(contentsOf: url), Data([0, 255]))
        XCTAssertEqual(url.lastPathComponent, "fixture.bin")
        XCTAssertNotNil(UUID(uuidString: url.deletingLastPathComponent().lastPathComponent))
        XCTAssertEqual(url.deletingLastPathComponent().deletingLastPathComponent().path, model.attachmentDirectory.path)
        XCTAssertTrue(model.downloadingAttachments.isEmpty)
        model.attachmentURL = nil
        core.suspendAttachment = false; core.fail = true
        await model.openAttachment(messageID: 42, part: part)
        XCTAssertNil(model.attachmentURL)
        XCTAssertNotNil(model.error)
        XCTAssertTrue(model.downloadingAttachments.isEmpty)
    }

    func testRowMappingKeepsFlagsDateAndFallbacks() {
        let row = MailRow(id: 42, account: "fixture", senderName: "", senderAddress: "sender@example.test", subject: "", preview: "Preview", dateUnix: 1_700_000_000, unread: true, hasAttachments: true, threadId: "thread-\(42)", messageCount: 1, messageIds: [42])
        let view = RowPresentation(row)
        XCTAssertEqual(view.sender, "sender@example.test")
        XCTAssertEqual(view.subject, "(No subject)")
        XCTAssertEqual(view.preview, "Preview")
        XCTAssertEqual(view.date.timeIntervalSince1970, 1_700_000_000)
        XCTAssertTrue(view.unread)
        XCTAssertTrue(view.hasAttachments)
    }

    func testNamedSenderAndReadMessage() {
        let row = MailRow(id: 43, account: "fixture", senderName: "Sender", senderAddress: "sender@example.test", subject: "Subject", preview: "", dateUnix: 0, unread: false, hasAttachments: false, threadId: "thread-\(43)", messageCount: 1, messageIds: [43])
        XCTAssertEqual(RowPresentation(row).sender, "Sender")
        XCTAssertEqual(RowPresentation(row).subject, "Subject")
        XCTAssertFalse(RowPresentation(row).unread)
    }

    func testReplyMappingKeepsCoreAccountAndThread() {
        let reply = ReplyDraft(account: "second-account", to: "sender@example.test", cc: "copy@example.test", subject: "Re: Subject", body: "\n\n> Quote", replyTo: 42, attachments: [])
        let draft = ComposeDraft(reply: reply)
        XCTAssertEqual(draft.account, "second-account")
        XCTAssertEqual(draft.to, reply.to)
        XCTAssertEqual(draft.cc, reply.cc)
        XCTAssertEqual(draft.subject, reply.subject)
        XCTAssertEqual(draft.body, reply.body)
        XCTAssertEqual(draft.replyTo, 42)
    }

    func testMailboxMapping() {
        XCTAssertEqual(MailboxChoice.inbox.mailbox, .inbox)
        XCTAssertEqual(MailboxChoice.later.mailbox, .later)
        XCTAssertEqual(MailboxChoice.done.mailbox, .done)
        XCTAssertEqual(MailboxChoice.sent.mailbox, .sent)
    }
}

@MainActor final class ComposeFixtureCore: LagomCore, @unchecked Sendable {
    override func laterChoices() -> [LaterChoice] {
        [LaterChoice(label: "Later: this evening 18:00", untilUnix: 18_000),
         LaterChoice(label: "Later: tomorrow 08:00", untilUnix: 86_400),
         LaterChoice(label: "Later: next week, Monday 08:00", untilUnix: 604_800)]
    }
    override func cancelBackgroundSync() {}
    override func backgroundSync() async throws -> [MailRow] { [] }
    override func refreshContacts(accountId: String) async throws {}
    override func suggestContacts(accountId: String, query: String) -> [ContactSuggestion] { [] }
    var attachmentCalls: [(Int64, String)] = []
    var suspendAttachment = false
    var attachmentGate: CheckedContinuation<Void, Never>?
    override func attachment(messageId: Int64, section: String) async throws -> Data {
        attachmentCalls.append((messageId, section))
        if suspendAttachment { await withCheckedContinuation { attachmentGate = $0 } }
        if fail { throw MailError.Failed(reason: "download failed") }
        return Data([0, 255])
    }
    var calls: [(Int64, ComposeKind)] = []
    var suspended = false
    var fail = false
    var continuation: CheckedContinuation<ReplyDraft, Error>?
    var sent: ReplyDraft?
    var reply = ReplyDraft(account: "source-account", to: "sender@example.test", cc: "copy@example.test",
                           subject: "Re: Fixture", body: "\n\n> Original", replyTo: 42, attachments: [])

    var queued: LocalDraft?
    override func syncAll() async throws {}
    override func undoSend(id: String) async throws -> Bool {
        guard let queued, queued.id == id else { return false }
        saved.append(queued); self.queued = nil
        return true
    }
    var snoozed: Int64?
    override func later(ids: [Int64], untilUnix: Int64) async throws -> [MailChange] { snoozed = untilUnix; return [] }
    var saved: [LocalDraft] = []
    var saveCalls = 0
    var saveGate: CheckedContinuation<Void, Never>?
    var suspendSave = false
    var restoredReply: LocalDraft?
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
        let rows = saved.map(\.row)
        if suspendDraftList { await withCheckedContinuation { draftListGate = $0 } }
        if failDraftList { throw MailError.Failed(reason: "draft list failed") }
        return rows
    }
    override func localDraft(id: String) async throws -> LocalDraft {
        guard let draft = saved.first(where: { $0.id == id }) else { throw MailError.NotFound }
        return draft
    }
    override func savedReply(id: Int64) async throws -> LocalDraft? { restoredReply }
    override func saveDraft(id: String, accountId: String, to: String, cc: String, bcc: String, subject: String, body: String,
                            replyTo: Int64?, attachments: [SendAttachment]) async throws -> DraftRow {
        saveCalls += 1
        if suspendSave { await withCheckedContinuation { saveGate = $0 } }
        if fail { throw MailError.Failed(reason: "fixture failure") }
        let draft = LocalDraft(id: id, account: accountId, to: to, cc: cc, subject: subject, body: body,
                               inReplyTo: replyTo.map { "<\($0)>" }, references: [], attachments: attachments, bcc: bcc)
        saved.removeAll { $0.id == id }
        saved.append(draft)
        return draft.row
    }
    override func deleteDraft(id: String) async throws {
        if fail { throw MailError.Failed(reason: "fixture failure") }
        saved.removeAll { $0.id == id }
    }
    override func sendDraft(id: String) async throws -> String {
        if fail { throw MailError.Failed(reason: "fixture failure") }
        let draft = saved.first { $0.id == id }!
        self.queued = draft
        let queued = try await send(accountId: draft.account, to: splitRecipients(value: draft.to), cc: splitRecipients(value: draft.cc),
            subject: draft.subject, bodyMarkdown: draft.body, replyTo: draft.inReplyTo == nil ? nil : 42, attachments: draft.attachments)
        saved.removeAll { $0.id == id }
        return queued
    }

    override func composeDraft(id: Int64, kind: ComposeKind) async throws -> ReplyDraft {
        calls.append((id, kind))
        if fail { throw MailError.Failed(reason: "fixture failure") }
        if suspended { return try await withCheckedThrowingContinuation { continuation = $0 } }
        return reply
    }

    override func send(accountId: String, to: [String], cc: [String], subject: String, bodyMarkdown: String,
                       replyTo: Int64?, attachments: [SendAttachment]) async throws -> String {
        if fail { throw MailError.Failed(reason: "fixture failure") }
        sent = ReplyDraft(account: accountId, to: to.joined(separator: ", "), cc: cc.joined(separator: ", "),
                          subject: subject, body: bodyMarkdown, replyTo: replyTo, attachments: attachments)
        return "queued"
    }
}

extension MailModelTests {
    @MainActor func testReplyCanTargetAnOlderConversationMember() async {
        let core = ComposeFixtureCore(noHandle: .init())
        let model = MailModel(); model.core = core; model.selectedID = 43
        await model.prepareCompose(id: 42, kind: .replyAll)
        XCTAssertEqual(core.calls.first?.0, 42)
        XCTAssertEqual(model.composeDraft?.replyTo, 42)
    }

    @MainActor func testReplyAllUsesMessageAccountAndCoreFields() async throws {
        let core = ComposeFixtureCore(noHandle: .init())
        let model = MailModel()
        model.core = core
        model.accountID = "another-account"
        model.selectedID = 42
        await model.prepareCompose(id: 42, kind: .replyAll)
        var draft = try XCTUnwrap(model.composeDraft)
        XCTAssertEqual(core.calls.first?.0, 42)
        XCTAssertEqual(core.calls.first?.1, .replyAll)
        XCTAssertEqual(draft.title, "Reply all")
        XCTAssertEqual(draft.account, "source-account")
        XCTAssertEqual(draft.replyTo, 42)
        XCTAssertEqual(draft.cc, "copy@example.test")
        XCTAssertEqual(draft.body, "\n\n> Original")
        draft.to = "First <first@example.test>; second@example.test"
        draft.cc = "\"Copy, Team\" <copy@example.test>"
        draft.body = "Answer\n" + draft.body
        try await draft.enqueue(using: core)
        XCTAssertEqual(core.sent?.to, "First <first@example.test>, second@example.test")
        XCTAssertEqual(core.sent?.cc, draft.cc)
        XCTAssertEqual(core.sent?.body, draft.body)
        XCTAssertEqual(core.sent?.replyTo, 42)
        XCTAssertFalse(model.preparingCompose)
    }

    @MainActor func testUndoRestoresDraftAndLaterUsesCoreChoices() async throws {
        let core = ComposeFixtureCore(noHandle: .init())
        let model = MailModel(); model.core = core
        model.composeDraft = ComposeDraft(reply: core.reply)
        let id = model.composeDraft?.messageID
        try await model.sendCompose()
        XCTAssertEqual(model.undoSendID, id)
        XCTAssertEqual(model.notice, "Sending…")
        await model.undoSend()
        XCTAssertEqual(model.composeDraft?.messageID, id)
        XCTAssertEqual(model.composeDraft?.body, core.reply.body)
        XCTAssertNil(model.undoSendID)
        for choice in core.laterChoices() {
            await model.act(.later, id: 42, until: choice.untilUnix)
            XCTAssertEqual(core.snoozed, choice.untilUnix)
        }
        await model.act(.later, id: 42)
        XCTAssertEqual(core.snoozed, 86_400)
        try await model.sendCompose()
        core.queued = nil
        await model.undoSend()
        XCTAssertEqual(model.notice, "Already sent")
    }

    @MainActor func testForwardKeepsAttachmentsAndHasNoReplyThread() async throws {
        let core = ComposeFixtureCore(noHandle: .init())
        core.reply.replyTo = nil
        core.reply.to = ""
        core.reply.cc = ""
        core.reply.subject = "Fwd: Fixture"
        core.reply.attachments = [SendAttachment(filename: "fixture.bin", mimeType: "application/octet-stream", data: Data([0, 255]))]
        let model = MailModel()
        model.core = core
        model.selectedID = 42
        await model.prepareCompose(id: 42, kind: .forward)
        var draft = try XCTUnwrap(model.composeDraft)
        XCTAssertEqual(draft.title, "Forward")
        XCTAssertNil(draft.replyTo)
        XCTAssertTrue(draft.to.isEmpty && draft.cc.isEmpty)
        draft.to = "next@example.test"
        try await draft.enqueue(using: core)
        XCTAssertNil(core.sent?.replyTo)
        XCTAssertEqual(core.sent?.attachments.first?.data, Data([0, 255]))
        XCTAssertEqual(core.sent?.attachments.first?.filename, "fixture.bin")
    }

    @MainActor func testSlowPreparationCannotOpenOnAnotherMessageOrRunTwice() async {
        let core = ComposeFixtureCore(noHandle: .init())
        core.suspended = true
        let model = MailModel()
        model.core = core
        model.selectedID = 42
        let pending = Task { await model.prepareCompose(id: 42, kind: .forward) }
        while core.continuation == nil { await Task.yield() }
        XCTAssertTrue(model.preparingCompose)
        await model.prepareCompose(id: 42, kind: .replyAll)
        XCTAssertEqual(core.calls.count, 1)
        model.selectedID = 43
        core.continuation?.resume(returning: core.reply)
        await pending.value
        XCTAssertNil(model.composeDraft)
        XCTAssertFalse(model.preparingCompose)
    }

    @MainActor func testPreparationPreservesAnExistingComposer() async {
        let core = ComposeFixtureCore(noHandle: .init())
        let model = MailModel()
        model.core = core
        model.selectedID = 42
        model.composeDraft = ComposeDraft(account: "chosen-account")
        model.composeDraft?.body = "Unsaved text"
        await model.prepareCompose(id: 42, kind: .reply)
        XCTAssertTrue(core.calls.isEmpty)
        XCTAssertEqual(model.composeDraft?.body, "Unsaved text")
    }

    @MainActor func testBccFromALinkIsSavedAndRestoredInItsField() async throws {
        let core = ComposeFixtureCore(noHandle: .init())
        let model = MailModel()
        model.core = core
        let mail = Mailto(to: ["a@example.test"], cc: [], bcc: ["hidden@example.test"], subject: "Hi", body: "")
        model.composeDraft = ComposeDraft(mailto: mail, account: "fixture")
        XCTAssertTrue(model.composeDraft?.showBcc == true)
        try await model.closeCompose(discard: false)
        let restored = ComposeDraft(saved: try XCTUnwrap(core.saved.first))
        XCTAssertEqual(restored.bcc, "hidden@example.test")
        XCTAssertTrue(restored.showBcc)
        XCTAssertFalse(ComposeDraft(account: "fixture").showBcc)
    }

    @MainActor func testPreparationAndEnqueueFailuresRetainEdits() async throws {
        let core = ComposeFixtureCore(noHandle: .init())
        core.fail = true
        let model = MailModel()
        model.core = core
        model.selectedID = 42
        await model.prepareCompose(id: 42, kind: .reply)
        XCTAssertNil(model.composeDraft)
        XCTAssertTrue(model.error?.contains("fixture failure") == true)
        XCTAssertFalse(model.preparingCompose)
        var draft = ComposeDraft(reply: core.reply)
        draft.body = "Unsaved answer"
        do {
            try await draft.enqueue(using: core)
            XCTFail("enqueue should fail")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("fixture failure"))
        }
        XCTAssertEqual(draft.body, "Unsaved answer")
        XCTAssertNil(core.sent)
    }
}

@MainActor final class DailyFixtureCore: LagomCore, @unchecked Sendable {
    override func laterChoices() -> [LaterChoice] {
        [LaterChoice(label: "Later: this evening 18:00", untilUnix: 18_000),
         LaterChoice(label: "Later: tomorrow 08:00", untilUnix: 86_400),
         LaterChoice(label: "Later: next week, Monday 08:00", untilUnix: 604_800)]
    }
    override func cancelBackgroundSync() {}
    override func backgroundSync() async throws -> [MailRow] { [] }
    var mail = (1...205).map { id in
        MailRow(id: Int64(id), account: id == 1 ? "second" : "fixture", senderName: "Sender",
                senderAddress: "sender@example.test", subject: "Mail", preview: "", dateUnix: Int64(id),
                unread: false, hasAttachments: false, threadId: "thread-\(Int64(id))", messageCount: 1, messageIds: [Int64(id)])
    }
    var queries: [(String?, String, UInt32)] = []
    var flags: [(Int64, Bool)] = []
    var failRead = false
    var suspendRead = false
    var readContinuation: CheckedContinuation<Void, Never>?
    var suspendSearch = false
    var searchContinuation: CheckedContinuation<[MailRow], Error>?
    var suspendInline = false
    var inlineContinuations: [Int64: CheckedContinuation<Void, Error>] = [:]
    var failInline = false

    override func accounts() async throws -> [MailAccount] { [] }
    override func sendFailures() async throws -> [SendFailure] { [] }
    override func localDrafts() async throws -> [DraftRow] { [] }
    override func nextSnooze() async throws -> Int64? { nil }
    override func mailbox(accountId: String?, mailbox: Mailbox, limit: UInt32, offset: UInt32) async throws -> [MailRow] {
        Array(mail.dropFirst(Int(offset)).prefix(Int(limit)))
    }
    override func search(accountId: String?, query: String, limit: UInt32, offset: UInt32) async throws -> [MailRow] {
        queries.append((accountId, query, offset))
        if suspendSearch { return try await withCheckedThrowingContinuation { searchContinuation = $0 } }
        return query == "none" ? [] : Array(mail.dropFirst(Int(offset)).prefix(Int(limit)))
    }
    var conversationIDs: [Int64] = []
    override func conversation(messageId: Int64) async throws -> [MailMessage] {
        var result: [MailMessage] = []
        for id in conversationIDs.isEmpty ? [messageId] : conversationIDs { result.append(try await message(id: id)) }
        return result
    }
    /// Text and HTML per message; the default is HTML with an inline image. `fetched` is what loadInline stores.
    var bodies: [Int64: (String, String?)] = [:]
    var fetched: [Int64: (String, String?)] = [:]
    var inlineCalls: [Int64] = []
    override func message(id: Int64) async throws -> MailMessage {
        let body = bodies[id] ?? ("body", "<img src='cid:logo'>")
        return MailMessage(row: mail.first { $0.id == id }!, textBody: body.0, htmlBody: body.1, attachments: [])
    }
    override func markRead(id: Int64, read: Bool) async throws {
        flags.append((id, read))
        if suspendRead { await withCheckedContinuation { readContinuation = $0 } }
        if failRead { throw MailError.Failed(reason: "flag refused") }
        mail[mail.firstIndex { $0.id == id }!].unread = !read
    }
    var unsubscribed: [Int64] = []
    override func unsubscribe(messageId: Int64) async throws {
        if messageId == 2 { throw MailError.Failed(reason: "the list could not be reached") }
        unsubscribed.append(messageId)
    }
    override func loadInline(id: Int64) async throws {
        inlineCalls.append(id)
        if failInline { throw MailError.Failed(reason: "inline unavailable") }
        if suspendInline { try await withCheckedThrowingContinuation { inlineContinuations[id] = $0 } }
        if let body = fetched.removeValue(forKey: id) { bodies[id] = body }
    }
}

extension MailModelTests {
    @MainActor func testUnsubscribeMarksOnlyWhatTheCoreConfirmed() async {
        let core = DailyFixtureCore(noHandle: .init())
        let model = MailModel()
        model.core = core
        await model.unsubscribe(id: 1)
        XCTAssertEqual(core.unsubscribed, [1])
        XCTAssertEqual(model.unsubscribedIDs, [1])
        await model.unsubscribe(id: 2)
        XCTAssertFalse(model.unsubscribedIDs.contains(2))
        XCTAssertEqual(model.error, "the list could not be reached")
        XCTAssertNil(model.unsubscribingID)
    }

    @MainActor func testSearchPagesAndClearingRestoresMailbox() async {
        let core = DailyFixtureCore(noHandle: .init())
        let model = MailModel()
        model.core = core
        model.searchText = "  mail  "
        await model.changeScope()
        XCTAssertNil(core.queries.first?.0)
        XCTAssertEqual(core.queries.first?.1, "mail")
        XCTAssertEqual(model.rows.count, 100)
        await model.reload(more: true)
        XCTAssertEqual(core.queries.last?.2, 100)
        XCTAssertEqual(model.rows.count, 200)
        await model.reload()
        XCTAssertEqual(model.rows.count, 200)
        await model.reload(more: true)
        XCTAssertEqual(model.rows.count, 205)
        XCTAssertFalse(model.hasMore)
        model.searchText = "none"
        await model.changeScope()
        XCTAssertTrue(model.rows.isEmpty)
        model.searchText = ""
        await model.changeScope()
        XCTAssertEqual(model.rows.count, 100)
        model.accountID = "second"
        model.searchText = "mail"
        await model.changeScope()
        XCTAssertEqual(core.queries.last?.0, "second")
    }

    @MainActor func testLateSearchCannotReplaceNewQuery() async {
        let core = DailyFixtureCore(noHandle: .init())
        core.suspendSearch = true
        let model = MailModel()
        model.core = core
        model.searchText = "old"
        let pending = Task { await model.changeScope() }
        while core.searchContinuation == nil { await Task.yield() }
        core.suspendSearch = false
        model.searchText = "none"
        await model.changeScope()
        core.searchContinuation?.resume(returning: [core.mail[0]])
        await pending.value
        XCTAssertTrue(model.rows.isEmpty)
        XCTAssertFalse(model.hasMore)
    }

    @MainActor func testCancelledSelectionCannotDiscardPendingSearch() async {
        let core = DailyFixtureCore(noHandle: .init())
        core.mail[0].unread = true
        core.suspendRead = true
        let model = MailModel()
        model.core = core
        model.selectedID = 1
        let selection = Task { await model.select(1) }
        while core.readContinuation == nil { await Task.yield() }
        core.suspendSearch = true
        model.searchText = "replacement"
        let search = Task { await model.changeScope() }
        while core.searchContinuation == nil { await Task.yield() }
        selection.cancel()
        core.readContinuation?.resume()
        await selection.value
        XCTAssertFalse(core.mail[0].unread)
        XCTAssertFalse(model.working)
        XCTAssertTrue(model.rows.isEmpty)
        core.searchContinuation?.resume(returning: [core.mail[1]])
        await search.value
        XCTAssertEqual(model.rows.map(\.id), [2])
        XCTAssertEqual(model.searchText, "replacement")
        XCTAssertEqual(core.queries.count, 1)
        XCTAssertNil(model.error)
    }

    @MainActor func testRemoteImagesResetOnSelectionAndIgnoreStaleRequests() async {
        let core = DailyFixtureCore(noHandle: .init())
        let model = MailModel()
        model.core = core
        model.selectedID = 1
        await model.select(1)
        XCTAssertTrue(model.remoteImagesIDs.isEmpty)
        XCTAssertEqual(model.inlineRevision, 1)
        model.loadRemoteImages(id: 1)
        XCTAssertEqual(model.remoteImagesIDs, [1])
        model.selectedID = 2
        await model.select(2)
        model.loadRemoteImages(id: 1)
        XCTAssertTrue(model.remoteImagesIDs.isEmpty)
        model.selectedID = 1
        await model.select(1)
        XCTAssertTrue(model.remoteImagesIDs.isEmpty)
        model.loadRemoteImages(id: 1)
        await model.changeScope()
        XCTAssertTrue(model.remoteImagesIDs.isEmpty)
    }

    @MainActor func testLateInlineLoadCannotRefreshAnotherMessage() async {
        let core = DailyFixtureCore(noHandle: .init())
        core.suspendInline = true
        let model = MailModel()
        model.core = core
        model.selectedID = 1
        let pending = Task { await model.select(1) }
        while core.inlineContinuations[1] == nil { await Task.yield() }
        core.suspendInline = false
        model.selectedID = 2
        await model.select(2)
        core.inlineContinuations[1]?.resume()
        await pending.value
        XCTAssertEqual(model.message?.row.id, 2)
        XCTAssertEqual(model.inlineRevision, 1)
    }

    @MainActor func testInlineFailureKeepsMessageAndRemoteImagesBlocked() async {
        let core = DailyFixtureCore(noHandle: .init())
        core.failInline = true
        let model = MailModel()
        model.core = core
        model.selectedID = 1
        await model.select(1)
        XCTAssertEqual(model.message?.row.id, 1)
        XCTAssertEqual(model.inlineRevision, 0)
        XCTAssertTrue(model.remoteImagesIDs.isEmpty)
        XCTAssertTrue(model.error?.contains("inline unavailable") == true)
    }

    @MainActor func testOpeningABodySyncLeftOutDownloadsItAndShowsTheHTML() async {
        let core = DailyFixtureCore(noHandle: .init())
        core.bodies[1] = ("The text of this message is larger than 4 MiB and was not downloaded.", nil)
        core.failInline = true
        let model = MailModel()
        model.core = core
        model.selectedID = 1
        await model.select(1)
        XCTAssertTrue(model.error?.hasPrefix("Could not download this message: ") == true)
        XCTAssertNil(model.message?.htmlBody)
        core.failInline = false
        core.fetched[1] = ("Full text", "<p>Full text</p>")
        model.error = nil
        await model.select(1)
        XCTAssertNil(model.error)
        XCTAssertEqual(model.message?.htmlBody, "<p>Full text</p>")
        XCTAssertEqual(model.messages.first?.textBody, "Full text")
        // One fetch per open: refreshing the reading view does not ask again.
        XCTAssertEqual(core.inlineCalls, [1, 1])
    }

    @MainActor func testReadActionsUpdateListAndDetailAndPreserveSearch() async {
        let core = DailyFixtureCore(noHandle: .init())
        let model = MailModel()
        model.core = core
        model.searchText = "mail"
        await model.reload()
        await model.reload(more: true)
        model.selectedID = 1
        await model.select(1)
        await model.setRead(id: 1, read: false)
        XCTAssertEqual(model.message?.row.unread, true)
        XCTAssertEqual(model.rows.first?.unread, true)
        XCTAssertEqual(model.rows.count, 200)
        XCTAssertEqual(core.queries.last?.1, "mail")
        await model.setRead(id: 1, read: true)
        XCTAssertEqual(model.message?.row.unread, false)
        XCTAssertEqual(model.rows.first?.unread, false)
        await model.setRead(id: 2, read: false)
        XCTAssertEqual(model.rows[1].unread, true)
        XCTAssertEqual(model.message?.row.id, 1)
        XCTAssertFalse(model.working)
    }

    @MainActor func testRejectedReadKeepsConfirmedState() async {
        let core = DailyFixtureCore(noHandle: .init())
        core.failRead = true
        let model = MailModel()
        model.core = core
        model.selectedID = 1
        await model.select(1)
        await model.setRead(id: 1, read: false)
        XCTAssertEqual(model.message?.row.unread, false)
        XCTAssertEqual(model.rows.first?.unread, false)
        XCTAssertTrue(model.error?.contains("flag refused") == true)
        XCTAssertFalse(model.working)
    }

    @MainActor func testSlowReadCannotReplaceSelectionOrRunTwice() async {
        let core = DailyFixtureCore(noHandle: .init())
        let model = MailModel()
        model.core = core
        model.selectedID = 1
        await model.select(1)
        core.suspendRead = true
        let pending = Task { await model.setRead(id: 1, read: false) }
        while core.readContinuation == nil { await Task.yield() }
        await model.setRead(id: 1, read: false)
        XCTAssertEqual(core.flags.count, 1)
        model.selectedID = 2
        await model.select(2)
        core.readContinuation?.resume()
        await pending.value
        XCTAssertEqual(model.message?.row.id, 2)
        XCTAssertEqual(model.rows.first?.unread, true)
        XCTAssertFalse(model.working)
    }

    @MainActor func testOpeningUnreadMailWhileAnotherFlagIsPendingStillMarksItRead() async {
        let core = DailyFixtureCore(noHandle: .init())
        core.mail[1].unread = true
        let model = MailModel()
        model.core = core
        core.suspendRead = true
        let pending = Task { await model.setRead(id: 1, read: false) }
        while core.readContinuation == nil { await Task.yield() }
        core.suspendRead = false
        model.selectedID = 2
        await model.select(2)
        XCTAssertEqual(model.message?.row.unread, false)
        XCTAssertTrue(model.working)
        core.readContinuation?.resume()
        await pending.value
        XCTAssertFalse(model.working)
        XCTAssertEqual(model.message?.row.id, 2)
        XCTAssertEqual(core.flags.map { $0.0 }, [1, 2])
    }


    @MainActor func testOpeningMessageDuringItsListReadActionShowsConfirmedFlag() async {
        let core = DailyFixtureCore(noHandle: .init())
        core.mail[0].unread = true
        core.suspendRead = true
        let model = MailModel()
        model.core = core
        let pending = Task { await model.setRead(id: 1, read: true) }
        while core.readContinuation == nil { await Task.yield() }
        model.selectedID = 1
        await model.select(1)
        XCTAssertEqual(model.message?.row.unread, true)
        core.readContinuation?.resume()
        await pending.value
        XCTAssertEqual(model.message?.row.unread, false)
        XCTAssertEqual(model.rows.first?.unread, false)
        XCTAssertEqual(core.flags.count, 1)
    }

}

extension MailModelTests {
    @MainActor func testDraftAutosaveDebouncesAndKeepFlushesLatestEdit() async throws {
        let core = ComposeFixtureCore(noHandle: .init())
        let model = MailModel(); model.core = core
        model.composeDraft = ComposeDraft(account: "fixture")
        model.composeDraft?.body = "first"
        try await Task.sleep(for: .milliseconds(300))
        model.composeDraft?.body = "latest"
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertEqual(core.saveCalls, 0)
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertEqual(core.saved.first?.body, "latest")
        model.composeDraft?.subject = "Last edit"
        try await model.closeCompose(discard: false)
        XCTAssertNil(model.composeDraft)
        XCTAssertEqual(core.saved.first?.subject, "Last edit")
        await model.openDraft(try XCTUnwrap(core.saved.first).row)
        XCTAssertEqual(model.composeDraft?.body, "latest")
        XCTAssertEqual(model.composeDraft?.subject, "Last edit")
    }

    @MainActor func testPickedAttachmentsWarnRemoveAutosaveAndRestore() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("\(UUID().uuidString).pdf")
        try Data([1, 2]).write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }
        let file = try readAttachment(url)
        XCTAssertEqual(file.mimeType, "application/pdf")
        let core = ComposeFixtureCore(noHandle: .init())
        let model = MailModel(); model.core = core
        model.composeDraft = ComposeDraft(account: "fixture")
        model.composeDraft?.attachments += [file, SendAttachment(filename: "big.bin", mimeType: "application/octet-stream", data: Data(count: attachmentWarningBytes))]
        XCTAssertEqual(model.composeDraft?.attachmentsTooLarge, true)
        model.composeDraft?.attachments.remove(at: 1)
        XCTAssertEqual(model.composeDraft?.attachmentsTooLarge, false)
        try await model.closeCompose(discard: false)
        XCTAssertEqual(core.saved.first?.attachments, [file])
        await model.openDraft(try XCTUnwrap(core.saved.first).row)
        XCTAssertEqual(model.composeDraft?.attachments, [file])
        try await model.closeCompose(discard: true)
    }

    @MainActor func testOpeningAStaleDraftRowReadsTheLatestSavedBody() async throws {
        let core = ComposeFixtureCore(noHandle: .init())
        let model = MailModel(); model.core = core
        model.composeDraft = ComposeDraft(account: "fixture")
        model.composeDraft?.body = "before"
        try await model.closeCompose(discard: false)
        let stale = try XCTUnwrap(core.saved.first).row
        core.saved[0].body = "latest on disk"
        await model.openDraft(stale)
        XCTAssertEqual(model.composeDraft?.body, "latest on disk")
        try await model.closeCompose(discard: true)
    }

    @MainActor func testChangingSenderKeepsEditsAndClearingTheBodyAutosaves() async throws {
        let core = ComposeFixtureCore(noHandle: .init())
        let model = MailModel(); model.core = core
        model.composeDraft = ComposeDraft(account: "first")
        model.composeDraft?.body = "keep edits"
        try await Task.sleep(for: .milliseconds(600))
        let sheetID = model.composeDraft?.id
        let old = model.composeDraft?.messageID
        try await model.changeComposeAccount("second")
        XCTAssertNotEqual(model.composeDraft?.messageID, old)
        XCTAssertEqual(model.composeDraft?.id, sheetID)
        XCTAssertEqual(core.saved.count, 1)
        XCTAssertEqual(core.saved.first?.account, "second")
        XCTAssertEqual(core.saved.first?.body, "keep edits")
        model.composeDraft?.body = ""
        try await Task.sleep(for: .milliseconds(600))
        XCTAssertEqual(core.saved.first?.body, "")
        try await model.closeCompose(discard: true)
    }

    @MainActor func testDiscardWaitsForInflightSaveAndCannotResurrectDraft() async throws {
        let core = ComposeFixtureCore(noHandle: .init()); core.suspendSave = true
        let model = MailModel(); model.core = core
        model.composeDraft = ComposeDraft(account: "fixture")
        model.composeDraft?.body = "discard me"
        while core.saveGate == nil { await Task.yield() }
        let close = Task { try await model.closeCompose(discard: true) }
        await Task.yield()
        XCTAssertNotNil(model.composeDraft)
        core.saveGate?.resume()
        try await close.value
        try await Task.sleep(for: .milliseconds(600))
        XCTAssertNil(model.composeDraft)
        XCTAssertTrue(core.saved.isEmpty)
    }

    @MainActor func testDraftFailuresRetainComposerAndReplyRestoresSavedFields() async throws {
        let core = ComposeFixtureCore(noHandle: .init())
        let model = MailModel(); model.core = core
        model.composeDraft = ComposeDraft(account: "fixture")
        model.composeDraft?.body = "keep me"
        core.fail = true
        for discard in [false, true] {
            do { try await model.closeCompose(discard: discard); XCTFail("Expected failure") } catch { }
            XCTAssertEqual(model.composeDraft?.body, "keep me")
        }
        core.fail = false
        try await model.closeCompose(discard: false)
        core.restoredReply = core.saved.first
        model.selectedID = 42
        await model.prepareCompose(id: 42, kind: .reply)
        XCTAssertEqual(model.composeDraft?.body, "keep me")
        XCTAssertTrue(core.calls.isEmpty)
        XCTAssertTrue(model.composeDraft?.restored == true)
        try await model.closeCompose(discard: true)
    }
}

private extension LocalDraft {
    var row: DraftRow { DraftRow(id: id, account: account, to: to, subject: subject, updatedUnix: 42) }
}

extension MailModelTests {
    func testDraftMessageIDUsesLastDomainAndCompactLowercaseUUID() {
        for (email, domain) in [("name@earlier@example.test", "example.test"), ("missing", "localhost")] {
            let id = draftMessageID(email: email)
            XCTAssertNotNil(id.range(of: "^<[0-9a-f]{32}@" + domain.replacingOccurrences(of: ".", with: "\\.") + ">$", options: .regularExpression))
            XCTAssertNotEqual(id, draftMessageID(email: email))
        }
    }

    @MainActor func testDraftListFailureDoesNotFailInboxReload() async {
        let core = ComposeFixtureCore(noHandle: .init()); core.failDraftList = true
        let model = MailModel(); model.core = core
        await model.reload()
        XCTAssertEqual(model.rows.map(\.id), [1])
        XCTAssertNotNil(model.error)
    }

    @MainActor func testDiscardDuringDraftListLoadDropsStaleRows() async throws {
        let core = ComposeFixtureCore(noHandle: .init())
        let model = MailModel(); model.core = core
        model.composeDraft = ComposeDraft(account: "fixture")
        model.composeDraft?.body = "saved"
        try await model.closeCompose(discard: false)
        await model.openDraft(try XCTUnwrap(model.drafts.first))
        core.suspendDraftList = true
        let load = Task { await model.reload() }
        while core.draftListGate == nil { await Task.yield() }
        try await model.closeCompose(discard: true)
        core.draftListGate?.resume()
        await load.value
        XCTAssertTrue(model.drafts.isEmpty)
        XCTAssertTrue(model.savedDraftIDs.isEmpty)
        XCTAssertEqual(model.rows.map(\.id), [1])
    }

    @MainActor func testOpeningDeletedDraftRemovesListRow() async throws {
        let core = ComposeFixtureCore(noHandle: .init())
        let model = MailModel(); model.core = core
        model.composeDraft = ComposeDraft(account: "fixture")
        model.composeDraft?.body = "saved"
        try await model.closeCompose(discard: false)
        let row = try XCTUnwrap(model.drafts.first)
        core.saved = []
        await model.openDraft(row)
        XCTAssertTrue(model.drafts.isEmpty)
        XCTAssertNil(model.composeDraft)
        XCTAssertNil(model.error)
    }
}

extension MailModelTests {
    @MainActor func testConversationExpansionReadStateAndRemoteConsentArePerMessage() async {
        let core = DailyFixtureCore(noHandle: .init())
        core.conversationIDs = [1, 2, 3]
        core.mail[1].unread = true
        let model = MailModel(); model.core = core; model.selectedID = 3
        await model.select(3)
        XCTAssertEqual(model.messages.map { $0.row.id }, [1, 2, 3])
        XCTAssertEqual(model.expandedIDs, [2, 3])
        XCTAssertEqual(core.flags.map { $0.0 }, [2])
        XCTAssertFalse(model.messages[1].row.unread)
        model.loadRemoteImages(id: 2)
        await model.toggleMessage(2)
        XCTAssertEqual(model.expandedIDs, [3])
        await model.setRead(id: 2, read: false)
        await model.toggleMessage(2)
        XCTAssertFalse(model.messages[1].row.unread)
        XCTAssertEqual(model.remoteImagesIDs, [2])
        await model.toggleMessage(1)
        XCTAssertEqual(model.expandedIDs, [1, 2, 3])
        XCTAssertEqual(core.flags.map { $0.0 }, [2, 2, 2])
        await model.changeScope()
        XCTAssertTrue(model.messages.isEmpty)
        XCTAssertTrue(model.expandedIDs.isEmpty)
        XCTAssertTrue(model.remoteImagesIDs.isEmpty)
    }
}
