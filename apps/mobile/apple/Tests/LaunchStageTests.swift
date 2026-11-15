import XCTest

final class LaunchStageTests: XCTestCase {
    func testStoreAndNotificationAnswerAreKnownBeforeAnythingShows() {
        XCTAssertEqual(LaunchStage(storeOpen: false, hasAccount: false, notificationsAsked: false, notificationsUndecided: true), .loading)
        XCTAssertEqual(LaunchStage(storeOpen: false, hasAccount: true, notificationsAsked: true, notificationsUndecided: false), .loading)
        // The system has not answered yet: neither the step nor the inbox flashes.
        XCTAssertEqual(LaunchStage(storeOpen: true, hasAccount: true, notificationsAsked: false, notificationsUndecided: nil), .loading)
        // Already allowed or denied in Settings: no step.
        XCTAssertEqual(LaunchStage(storeOpen: true, hasAccount: true, notificationsAsked: false, notificationsUndecided: false), .app)
    }

    /// The gate as `LagomApp` feeds it: no account, the intro; one added, the step once; the last removed, the intro.
    @MainActor func testIntroThenNotificationsOnceThenInboxAndBackToIntro() async {
        let model = MailModel()
        var asked = false
        let stage = { LaunchStage(storeOpen: model.core != nil, hasAccount: !model.accounts.isEmpty,
                                  notificationsAsked: asked, notificationsUndecided: true) }
        XCTAssertEqual(stage(), .loading)
        let core = AccountsFixtureCore(noHandle: .init())
        model.core = core
        await model.reload()
        XCTAssertEqual(stage(), .intro)
        // Add account and Sign in reload once the core has the account.
        let account = MailAccount(id: "fixture", email: "me@example.test", displayName: "Me")
        core.list = [account]
        await model.reload()
        XCTAssertEqual(stage(), .notifications)
        asked = true
        XCTAssertEqual(stage(), .app)
        await model.remove(account)
        XCTAssertTrue(core.list.isEmpty)
        XCTAssertEqual(stage(), .intro)
        // A second account later goes straight to the inbox.
        core.list = [account]
        await model.reload()
        XCTAssertEqual(stage(), .app)
    }
}

@MainActor final class AccountsFixtureCore: LagomCore, @unchecked Sendable {
    var list: [MailAccount] = []
    override func accounts() async throws -> [MailAccount] { list }
    override func removeAccount(id: String) async throws { list.removeAll { $0.id == id } }
    override func folders(accountId: String?) async throws -> [MailFolder] { [] }
    override func nextSnooze() async throws -> Int64? { nil }
    override func sendFailures() async throws -> [SendFailure] { [] }
    override func localDrafts() async throws -> [DraftRow] { [] }
    override func mailbox(accountId: String?, mailbox: Mailbox, limit: UInt32, offset: UInt32) async throws -> [MailRow] { [] }
}
