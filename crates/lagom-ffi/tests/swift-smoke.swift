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
        try await core.syncAll()
        do {
            _ = try await core.message(id: -1)
            preconditionFailure("missing message did not throw")
        } catch MailError.NotFound { }

        let recipients = splitRecipients(value: "\"Example, Sender\" <sender@example.test>, next@example.test")
        precondition(recipients.count == 2)
        let tomorrow = try tomorrowAtEight()
        precondition(tomorrow > Int64(Date().timeIntervalSince1970))
        precondition(mailDocument(html: "<LINK rel=preconnect href='https://example.test'>", dark: true, messageId: 1, remote: false).contains("<x-LINK"))
        print("Swift FFI smoke: 7 checks passed")
    }
}
