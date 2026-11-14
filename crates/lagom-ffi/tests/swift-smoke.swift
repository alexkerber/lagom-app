import Foundation

// Run against the host static library: no app, simulator, network or keychain.
@main
struct Smoke {
    static func main() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let core = try LagomCore.open(dataDir: directory.path)
        let accounts = try await core.accounts()
        let inbox = try await core.inbox(limit: 100, offset: 0)
        precondition(accounts.isEmpty && inbox.isEmpty)
        precondition(core.suggestContacts(accountId: "missing", query: "sender").isEmpty)
        do {
            try await core.refreshContacts(accountId: "missing")
            preconditionFailure("missing account did not throw")
        } catch MailError.NotFound { }
        try await core.syncAll()
        let backgroundRows = try await core.backgroundSync()
        precondition(backgroundRows.isEmpty)
        core.cancelBackgroundSync()
        // A cancelled handle stops before the next account and returns what it found.
        let cancelledRows = try await core.backgroundSync()
        precondition(cancelledRows.isEmpty, "cancelled background sync returned rows")
        do {
            _ = try await core.conversation(messageId: -1)
            preconditionFailure("missing message did not throw")
        } catch MailError.NotFound { }

        do {
            _ = try await core.message(id: -1)
            preconditionFailure("missing message did not throw")
        } catch MailError.NotFound { }
        let recipients = splitRecipients(value: "\"Example, Sender\" <sender@example.test>, next@example.test")
        precondition(recipients.count == 2)
        let choices = core.laterChoices()
        precondition(choices.map(\.label) == ["Later: this evening 18:00", "Later: tomorrow 08:00", "Later: next week, Monday 08:00"])
        precondition(choices.allSatisfy { $0.untilUnix > Int64(Date().timeIntervalSince1970) })
        let undone = try await core.undoSend(id: "missing")
        precondition(!undone)
        precondition(mailDocument(html: "<LINK rel=preconnect href='https://example.test'>", dark: true, messageId: 1, remote: false).contains("<x-LINK"))
        // Browser sign-in: only the pending sign-in's own redirect ends it, and only once.
        let stray = "msauth.com.example.app://auth?code=stray&state=guess"
        do { _ = try await core.finishSignIn(callbackUrl: stray); preconditionFailure("redirect without a sign-in did not throw") } catch MailError.Failed { }
        configureSignIn(appId: "com.example.app", googleClientId: "", microsoftClientId: "fixture-client")
        precondition(!signInAvailable(provider: .google) && signInAvailable(provider: .microsoft))
        let start = try core.beginSignIn(provider: .microsoft)
        precondition(start.callbackScheme == "msauth.com.example.app")
        let query = URLComponents(string: start.url)?.queryItems ?? []
        let state = query.first { $0.name == "state" }?.value ?? ""
        precondition(query.contains { $0.name == "redirect_uri" && $0.value == "msauth.com.example.app://auth" })
        precondition(query.contains { $0.name == "code_challenge_method" && $0.value == "S256" } && state.count >= 32)
        do { _ = try await core.finishSignIn(callbackUrl: stray); preconditionFailure("wrong state did not throw") } catch MailError.Failed { }
        let denied = "msauth.com.example.app://auth?error=access_denied&state=\(state)"
        do { _ = try await core.finishSignIn(callbackUrl: denied); preconditionFailure("denied sign-in did not throw") }
        catch MailError.Failed(let reason) { precondition(reason == "sign-in cancelled", "a wrong state ended the sign-in") }
        do { _ = try await core.finishSignIn(callbackUrl: denied); preconditionFailure("reused redirect did not throw") }
        catch MailError.Failed(let reason) { precondition(reason != "sign-in cancelled") }
        print("Swift FFI smoke: 19 checks passed")
    }
}
