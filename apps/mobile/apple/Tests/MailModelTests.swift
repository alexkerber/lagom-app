import XCTest

final class MailModelTests: XCTestCase {
    func testRowMappingKeepsFlagsDateAndFallbacks() {
        let row = MailRow(id: 42, account: "fixture", senderName: "", senderAddress: "sender@example.test", subject: "", preview: "Preview", dateUnix: 1_700_000_000, unread: true, hasAttachments: true)
        let view = RowPresentation(row)
        XCTAssertEqual(view.sender, "sender@example.test")
        XCTAssertEqual(view.subject, "(No subject)")
        XCTAssertEqual(view.preview, "Preview")
        XCTAssertEqual(view.date.timeIntervalSince1970, 1_700_000_000)
        XCTAssertTrue(view.unread)
        XCTAssertTrue(view.hasAttachments)
    }

    func testNamedSenderAndReadMessage() {
        let row = MailRow(id: 43, account: "fixture", senderName: "Sender", senderAddress: "sender@example.test", subject: "Subject", preview: "", dateUnix: 0, unread: false, hasAttachments: false)
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
    var calls: [(Int64, ComposeKind)] = []
    var suspended = false
    var fail = false
    var continuation: CheckedContinuation<ReplyDraft, Error>?
    var sent: ReplyDraft?
    var reply = ReplyDraft(account: "source-account", to: "sender@example.test", cc: "copy@example.test",
                           subject: "Re: Fixture", body: "\n\n> Original", replyTo: 42, attachments: [])

    override func composeDraft(id: Int64, kind: ComposeKind) async throws -> ReplyDraft {
        calls.append((id, kind))
        if fail { throw MailError.Failed(message: "fixture failure") }
        if suspended { return try await withCheckedThrowingContinuation { continuation = $0 } }
        return reply
    }

    override func send(accountId: String, to: [String], cc: [String], subject: String, bodyMarkdown: String,
                       replyTo: Int64?, attachments: [SendAttachment]) async throws -> String {
        if fail { throw MailError.Failed(message: "fixture failure") }
        sent = ReplyDraft(account: accountId, to: to.joined(separator: ", "), cc: cc.joined(separator: ", "),
                          subject: subject, body: bodyMarkdown, replyTo: replyTo, attachments: attachments)
        return "queued"
    }
}

extension MailModelTests {
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
    var mail = (1...205).map { id in
        MailRow(id: Int64(id), account: id == 1 ? "second" : "fixture", senderName: "Sender",
                senderAddress: "sender@example.test", subject: "Mail", preview: "", dateUnix: Int64(id),
                unread: false, hasAttachments: false)
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
    override func nextSnooze() async throws -> Int64? { nil }
    override func mailbox(accountId: String?, mailbox: Mailbox, limit: UInt32, offset: UInt32) async throws -> [MailRow] {
        Array(mail.dropFirst(Int(offset)).prefix(Int(limit)))
    }
    override func search(accountId: String?, query: String, limit: UInt32, offset: UInt32) async throws -> [MailRow] {
        queries.append((accountId, query, offset))
        if suspendSearch { return try await withCheckedThrowingContinuation { searchContinuation = $0 } }
        return query == "none" ? [] : Array(mail.dropFirst(Int(offset)).prefix(Int(limit)))
    }
    override func message(id: Int64) async throws -> MailMessage {
        MailMessage(row: mail.first { $0.id == id }!, textBody: "body", htmlBody: "<img src='cid:logo'>", attachments: [])
    }
    override func markRead(id: Int64, read: Bool) async throws {
        flags.append((id, read))
        if suspendRead { await withCheckedContinuation { readContinuation = $0 } }
        if failRead { throw MailError.Failed(message: "flag refused") }
        mail[mail.firstIndex { $0.id == id }!].unread = !read
    }
    override func loadInline(id: Int64) async throws {
        if failInline { throw MailError.Failed(message: "inline unavailable") }
        if suspendInline { try await withCheckedThrowingContinuation { inlineContinuations[id] = $0 } }
    }
}

extension MailModelTests {
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

    @MainActor func testRemoteImagesResetOnSelectionAndIgnoreStaleRequests() async {
        let core = DailyFixtureCore(noHandle: .init())
        let model = MailModel()
        model.core = core
        model.selectedID = 1
        await model.select(1)
        XCTAssertNil(model.remoteImagesID)
        XCTAssertEqual(model.inlineRevision, 1)
        model.loadRemoteImages(id: 1)
        XCTAssertEqual(model.remoteImagesID, 1)
        model.selectedID = 2
        await model.select(2)
        model.loadRemoteImages(id: 1)
        XCTAssertNil(model.remoteImagesID)
        model.selectedID = 1
        await model.select(1)
        XCTAssertNil(model.remoteImagesID)
        model.loadRemoteImages(id: 1)
        await model.changeScope()
        XCTAssertNil(model.remoteImagesID)
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
        XCTAssertNil(model.remoteImagesID)
        XCTAssertTrue(model.error?.contains("inline unavailable") == true)
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
