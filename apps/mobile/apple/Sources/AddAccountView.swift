import AuthenticationServices
import SwiftUI

struct AddAccountView: View {
    let model: MailModel
    /// Starts on Choose your mail, as the intro; false opens the address form at once (the intro's Other).
    var providers = true
    @Environment(\.dismiss) private var dismiss
    @State private var form = false
    @State private var email = ""
    @State private var password = ""
    @State private var passwordStep = false
    @State private var advanced = false
    @State private var imapHost = ""
    @State private var smtpHost = ""
    @State private var imapPort = "993"
    @State private var smtpPort = "587"
    @State private var imapUser = ""
    @State private var smtpUser = ""
    @State private var imapTLS = true
    @State private var smtpTLS = false
    @State private var connecting = false
    @State private var error: String?

    var body: some View {
        SheetScaffold(title: "Add account", cancelDisabled: connecting, cancel: { password = ""; dismiss() },
                      confirm: !providers || form ? SheetConfirm(title: passwordStep ? "Sign in" : "Continue", systemImage: passwordStep ? "checkmark" : "arrow.right", disabled: email.isEmpty || connecting || (passwordStep && password.isEmpty)) {
                          if passwordStep { Task { await connect() } }
                          else { passwordStep = true; imapUser = email; smtpUser = email }
                      } : nil) {
            if providers && !form {
                ScrollView {
                    ChooseMail(model: model, connecting: $connecting, signedIn: { dismiss() }) { form = true }
                        .padding(.horizontal, 16).padding(.bottom, 16)
                        .frame(maxWidth: 440, alignment: .leading).frame(maxWidth: .infinity)
                }
            } else {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    FieldRow(label: "Email") {
                        TextField("you@example.com", text: $email).textContentType(.emailAddress).keyboardType(.emailAddress)
                            .textInputAutocapitalization(.never).autocorrectionDisabled().accessibilityLabel("Email")
                    }
                    if passwordStep {
                        FieldRow(label: "Password") {
                            SecureField("App password", text: $password).textContentType(.password).accessibilityLabel("Password")
                        }
                        Text(signInAvailable(provider: .microsoft)
                            ? "Gmail and iCloud need an app password, not your normal password. For Microsoft, go back and choose Continue with Microsoft."
                            : "Gmail and iCloud need an app password, not your normal password. Some Microsoft accounts only work with Sign in with Microsoft, which this version does not have.")
                            .lagomFont(.small).foregroundStyle(Theme.secondary).padding(.vertical, 12)
                        Toggle("Advanced", isOn: $advanced).lagomFont(.base).frame(minHeight: 44)
                        if advanced {
                            server("Incoming mail", example: "imap.example.com", host: $imapHost, port: $imapPort, username: $imapUser, tls: $imapTLS)
                            server("Outgoing mail", example: "smtp.example.com", host: $smtpHost, port: $smtpPort, username: $smtpUser, tls: $smtpTLS)
                        }
                    }
                    if let error { Text(error).lagomFont(.snippet).foregroundStyle(Theme.danger).padding(.vertical, 12) }
                }
                .padding(.horizontal, 16)
            }
            }
        } footer: {
            if !providers || form {
            if providers {
                BackButton { back() }
                    .disabled(connecting)
            }
            if connecting {
                ProgressView()
                Text("Signing in…").lagomFont(.small).foregroundStyle(Theme.secondary)
            }
            }
            Spacer(minLength: 0)
        }
        .disabled(connecting)
        .interactiveDismissDisabled(connecting)
    }

    private func server(_ title: String, example: String, host: Binding<String>, port: Binding<String>, username: Binding<String>, tls: Binding<Bool>) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            GroupHeader(title: title).padding(.horizontal, -10)
            FieldRow(label: "Server") {
                TextField(example, text: host).textInputAutocapitalization(.never).autocorrectionDisabled().accessibilityLabel("\(title) server")
            }
            FieldRow(label: "Port") { TextField("", text: port).keyboardType(.numberPad).accessibilityLabel("\(title) port") }
            FieldRow(label: "Username") {
                TextField("", text: username).textInputAutocapitalization(.never).autocorrectionDisabled().accessibilityLabel("\(title) username")
            }
            FieldRow(label: "Security") {
                Picker("\(title) security", selection: tls) {
                    Text("TLS").tag(true)
                    Text("STARTTLS").tag(false)
                }
                .pickerStyle(.segmented).fixedSize()
            }
        }
    }

    /// Back to the list starts the form afresh, so one kind's servers never meet the next kind's password.
    private func back() {
        form = false; passwordStep = false; advanced = false; error = nil
        email = ""; password = ""; imapUser = ""; smtpUser = ""
        imapHost = ""; smtpHost = ""; imapPort = "993"; smtpPort = "587"; imapTLS = true; smtpTLS = false
    }

    private func connect() async {
        guard let core = model.core else { return }
        connecting = true
        defer { connecting = false }
        do {
            if advanced {
                guard let imap = UInt16(imapPort), let smtp = UInt16(smtpPort) else {
                    error = "Ports must be numbers from 1 to 65535."
                    return
                }
                _ = try await core.addAccountManual(email: email, password: password,
                    imap: MailServer(host: imapHost, port: imap, username: imapUser, security: imapTLS ? .tls : .startTls),
                    smtp: MailServer(host: smtpHost, port: smtp, username: smtpUser, security: smtpTLS ? .tls : .startTls))
            } else {
                _ = try await core.addAccount(email: email, password: password)
            }
            password = ""
            await model.reload()
            dismiss()
            await model.sync()
        } catch { self.error = error.localizedDescription }
    }
}

extension MailAccount: Identifiable {}

extension MailModel {
    /// Signs in on the provider's own page and adds the account; false when the user closed the page.
    /// `exchanging` runs once the page has returned, while the core trades its code for tokens.
    func signIn(_ provider: SignInProvider, browser: WebAuthenticationSession, exchanging: () -> Void) async throws -> Bool {
        guard let core else { return false }
        if let mock = Storage.mockSignIn, provider == .google { return try await mockSignIn(mock, exchanging: exchanging) }
        return try await authenticate(core.beginSignIn(provider: provider), browser: browser, exchanging: exchanging)
    }

    /// Adds Google Calendar and Tasks to a Google account on Google's own page; false when the user closed it.
    func allowCalendar(_ account: String, browser: WebAuthenticationSession, exchanging: () -> Void) async throws -> Bool {
        guard let core else { return false }
        return try await authenticate(core.beginCalendarAccess(accountId: account), browser: browser, exchanging: exchanging)
    }

    /// `Storage.mockSignIn`: page and exchange take two seconds; ok seeds the demo's
    /// Google accounts (mail scopes only, so Calendar offers its grant).
    private func mockSignIn(_ outcome: String, exchanging: () -> Void) async throws -> Bool {
        // At once, unlike the real page: the page's modal sheet is what keeps Back and the rows from a second tap.
        exchanging()
        try await Task.sleep(for: .seconds(2))
        if outcome == "cancel" { return false }
        if outcome == "error" { throw MailError.Failed(reason: "Google did not accept the sign-in. Try again.") }
        try seedDemoWithoutCalendar(dataDir: Storage.directory().path, state: "google")
        await reload()
        return true
    }

    private func authenticate(_ start: SignInStart, browser: WebAuthenticationSession, exchanging: () -> Void) async throws -> Bool {
        guard let core else { return false }
        guard let url = URL(string: start.url) else { return false }
        let redirect: URL
        do {
            // Ephemeral: no cookies shared with Safari and no system prompt. The core checks the redirect's state.
            redirect = try await browser.authenticate(using: url, callback: .customScheme(start.callbackScheme),
                preferredBrowserSession: .ephemeral, additionalHeaderFields: [:])
        } catch let stopped as ASWebAuthenticationSessionError where stopped.code == .canceledLogin { return false }
        exchanging()
        _ = try await core.finishSignIn(callbackUrl: redirect.absoluteString)
        await reload()
        return true
    }
}

/// The desktop's per-account signature: added to new mail, replies and forwards.
struct SignatureView: View {
    let model: MailModel
    let account: MailAccount
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    @State private var loaded = false
    @State private var saving = false
    @State private var error: String?

    var body: some View {
        SheetScaffold(title: "Signature", cancelDisabled: saving, cancel: { dismiss() },
                      confirm: SheetConfirm(title: "Save", systemImage: "checkmark", disabled: !loaded || saving) { Task { await save() } }) {
            VStack(alignment: .leading, spacing: 0) {
                FieldRow(label: "Account") { Text(account.email).lineLimit(1) }
                    .padding(.horizontal, 16)
                TextEditor(text: $text)
                    .lagomFont(.body).lineSpacing(3)
                    .scrollContentBackground(.hidden)
                    .overlay(alignment: .topLeading) {
                        if text.isEmpty {
                            Text("Added to new mail, replies and forwards.").lagomFont(.body).foregroundStyle(Theme.secondary)
                                .padding(.top, 8).padding(.leading, 5).allowsHitTesting(false).accessibilityHidden(true)
                        }
                    }
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    .disabled(!loaded)
                    .accessibilityLabel("Signature")
                if let error { Text(error).lagomFont(.snippet).foregroundStyle(Theme.danger).padding(16) }
            }
        }
        .interactiveDismissDisabled(saving)
        .task {
            guard let core = model.core else { return }
            do {
                text = try await core.signature(accountId: account.id)
                loaded = true
            } catch { self.error = error.localizedDescription }
        }
    }

    private func save() async {
        guard let core = model.core else { return }
        saving = true
        defer { saving = false }
        do {
            try await core.setSignature(accountId: account.id, text: text)
            dismiss()
        } catch { self.error = error.localizedDescription }
    }
}
