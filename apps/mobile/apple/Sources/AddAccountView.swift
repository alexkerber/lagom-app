import AuthenticationServices
import SwiftUI

struct AddAccountView: View {
    let model: MailModel
    @Environment(\.dismiss) private var dismiss
    @Environment(\.webAuthenticationSession) private var browser
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
        SheetScaffold(title: "Add account", cancelDisabled: connecting, cancel: { password = ""; dismiss() }) {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    // Hidden until this build has the provider's client ID.
                    ForEach([(SignInProvider.google, "Google"), (.microsoft, "Microsoft")], id: \.1) { provider, name in
                        if signInAvailable(provider: provider) {
                            Button("Sign in with \(name)") { Task { await signIn(provider) } }
                                .buttonStyle(HouseButtonStyle()).frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                    FieldRow(label: "Email") {
                        TextField("you@example.com", text: $email).textContentType(.emailAddress).keyboardType(.emailAddress)
                            .textInputAutocapitalization(.never).autocorrectionDisabled().accessibilityLabel("Email")
                    }
                    if passwordStep {
                        FieldRow(label: "Password") {
                            SecureField("App password", text: $password).textContentType(.password).accessibilityLabel("Password")
                        }
                        Text(signInAvailable(provider: .microsoft)
                            ? "Gmail and iCloud need an app password, not your normal password. For Microsoft, use Sign in with Microsoft."
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
        } footer: {
            HouseButton(title: passwordStep ? "Sign in" : "Continue", icon: "account-add", primary: true) {
                if passwordStep { Task { await connect() } }
                else { passwordStep = true; imapUser = email; smtpUser = email }
            }
            .disabled(email.isEmpty || connecting || (passwordStep && password.isEmpty))
            if connecting {
                ProgressView()
                Text("Signing in…").lagomFont(.small).foregroundStyle(Theme.secondary)
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

    private func signIn(_ provider: SignInProvider) async {
        guard let core = model.core else { return }
        error = nil
        do {
            let start = try core.beginSignIn(provider: provider)
            guard let url = URL(string: start.url) else { return }
            // Ephemeral: no cookies shared with Safari and no system prompt. The core checks the redirect's state.
            let redirect = try await browser.authenticate(using: url, callback: .customScheme(start.callbackScheme),
                preferredBrowserSession: .ephemeral, additionalHeaderFields: [:])
            connecting = true
            defer { connecting = false }
            _ = try await core.finishSignIn(callbackUrl: redirect.absoluteString)
            await model.reload()
            dismiss()
            await model.sync()
        } catch let stopped as ASWebAuthenticationSessionError where stopped.code == .canceledLogin {
            // The user closed the browser sheet.
        } catch { self.error = error.localizedDescription }
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
        SheetScaffold(title: "Signature", cancelDisabled: saving, cancel: { dismiss() }) {
            VStack(alignment: .leading, spacing: 0) {
                FieldRow(label: "Account") { Text(account.email).lineLimit(1) }
                    .padding(.horizontal, 16)
                TextEditor(text: $text)
                    .lagomFont(.body).lineSpacing(3)
                    .scrollContentBackground(.hidden)
                    .overlay(alignment: .topLeading) {
                        if text.isEmpty {
                            Text("Added to new mail, replies and forwards. Markdown supported.").lagomFont(.body).foregroundStyle(Theme.secondary)
                                .padding(.top, 8).padding(.leading, 5).allowsHitTesting(false).accessibilityHidden(true)
                        }
                    }
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    .disabled(!loaded)
                    .accessibilityLabel("Signature")
                if let error { Text(error).lagomFont(.snippet).foregroundStyle(Theme.danger).padding(16) }
            }
        } footer: {
            HouseButton(title: "Save", icon: "done", primary: true) { Task { await save() } }
                .disabled(!loaded || saving)
            Spacer(minLength: 0)
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
