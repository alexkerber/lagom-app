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
