import SwiftUI

struct AddAccountView: View {
    let model: MailModel
    @Environment(\.dismiss) private var dismiss
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
                    FieldRow(label: "Email") {
                        TextField("you@example.com", text: $email).textContentType(.emailAddress).keyboardType(.emailAddress)
                            .textInputAutocapitalization(.never).autocorrectionDisabled().accessibilityLabel("Email")
                    }
                    if passwordStep {
                        FieldRow(label: "Password") {
                            SecureField("App password", text: $password).textContentType(.password).accessibilityLabel("Password")
                        }
                        Text("Gmail and iCloud require an app-specific password. Microsoft accounts may require OAuth, which is not available in this version.")
                            .lagomFont(.small).foregroundStyle(Theme.secondary).padding(.vertical, 12)
                        // shortcut: Google/Microsoft OAuth on iOS needs per-platform client IDs; v1 uses passwords.
                        Toggle("Advanced", isOn: $advanced).lagomFont(.base).frame(minHeight: 44)
                        if advanced {
                            server("IMAP", host: $imapHost, port: $imapPort, username: $imapUser, tls: $imapTLS)
                            server("SMTP", host: $smtpHost, port: $smtpPort, username: $smtpUser, tls: $smtpTLS)
                        }
                    }
                    if let error { Text(error).lagomFont(.snippet).foregroundStyle(Theme.danger).padding(.vertical, 12) }
                }
                .padding(.horizontal, 16)
            }
        } footer: {
            HouseButton(title: passwordStep ? "Connect" : "Continue", icon: "account-add", primary: true) {
                if passwordStep { Task { await connect() } }
                else { passwordStep = true; imapUser = email; smtpUser = email }
            }
            .disabled(email.isEmpty || connecting || (passwordStep && password.isEmpty))
            if connecting {
                ProgressView()
                Text("Connecting…").lagomFont(.small).foregroundStyle(Theme.secondary)
            }
            Spacer(minLength: 0)
        }
        .disabled(connecting)
        .interactiveDismissDisabled(connecting)
    }

    private func server(_ title: String, host: Binding<String>, port: Binding<String>, username: Binding<String>, tls: Binding<Bool>) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            GroupHeader(title: title).padding(.horizontal, -10)
            FieldRow(label: "Host") {
                TextField("", text: host).textInputAutocapitalization(.never).autocorrectionDisabled().accessibilityLabel("\(title) host")
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
