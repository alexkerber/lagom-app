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
        NavigationStack {
            Form {
                TextField("Email", text: $email).textContentType(.emailAddress).keyboardType(.emailAddress)
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                if passwordStep {
                    SecureField("Password", text: $password).textContentType(.password)
                    Text("Gmail and iCloud require an app-specific password. Microsoft accounts may require OAuth, which is not available in this version.")
                        .font(.footnote).foregroundStyle(Theme.secondary)
                    // shortcut: Google/Microsoft OAuth on iOS needs per-platform client IDs; v1 uses passwords.
                    Toggle("Advanced", isOn: $advanced)
                    if advanced {
                        Section("IMAP") {
                            serverFields(host: $imapHost, port: $imapPort, username: $imapUser, tls: $imapTLS)
                        }
                        Section("SMTP") {
                            serverFields(host: $smtpHost, port: $smtpPort, username: $smtpUser, tls: $smtpTLS)
                        }
                    }
                }
                if let error { Text(error).foregroundStyle(Theme.danger) }
                if connecting { ProgressView("Connecting…") }
            }
            .navigationTitle("Add account")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { password = ""; dismiss() }.disabled(connecting) }
                ToolbarItem(placement: .confirmationAction) {
                    Button(passwordStep ? "Connect" : "Continue") {
                        if passwordStep { Task { await connect() } }
                        else { passwordStep = true; imapUser = email; smtpUser = email }
                    }.disabled(email.isEmpty || connecting || (passwordStep && password.isEmpty))
                }
            }
            .disabled(connecting)
            .interactiveDismissDisabled(connecting)
        }
    }

    private func serverFields(host: Binding<String>, port: Binding<String>, username: Binding<String>, tls: Binding<Bool>) -> some View {
        Group {
            TextField("Host", text: host).textInputAutocapitalization(.never).autocorrectionDisabled()
            TextField("Port", text: port).keyboardType(.numberPad)
            TextField("Username", text: username).textInputAutocapitalization(.never).autocorrectionDisabled()
            Picker("Security", selection: tls) {
                Text("TLS").tag(true)
                Text("STARTTLS").tag(false)
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
