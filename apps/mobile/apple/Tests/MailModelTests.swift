import XCTest
@testable import Lagom

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
        let reply = ReplyDraft(account: "second-account", to: "sender@example.test", subject: "Re: Subject", body: "\n\n> Quote", replyTo: 42)
        let draft = ComposeDraft(reply: reply)
        XCTAssertEqual(draft.account, "second-account")
        XCTAssertEqual(draft.to, reply.to)
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
