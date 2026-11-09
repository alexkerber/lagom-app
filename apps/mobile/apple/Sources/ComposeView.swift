import SwiftUI

struct ComposeDraft: Identifiable {
    let id = UUID()
    var account: String
    var to = ""
    var subject = ""
    var body = ""
    var replyTo: Int64?

    init(account: String) { self.account = account }
    init(reply: ReplyDraft) {
        account = reply.account
        to = reply.to
        subject = reply.subject
        body = reply.body
        replyTo = reply.replyTo
    }
}

struct ComposeView: View {
    let model: MailModel
    @State var draft: ComposeDraft
    @State private var sending = false
    @State private var error: String?
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Picker("From", selection: $draft.account) {
                    ForEach(model.accounts, id: \.id) { Text($0.email).tag($0.id) }
                }.disabled(draft.replyTo != nil)
                TextField("To", text: $draft.to).textContentType(.emailAddress).keyboardType(.emailAddress).textInputAutocapitalization(.never).autocorrectionDisabled()
                TextField("Subject", text: $draft.subject)
                Section("Message · Markdown supported") {
                    TextEditor(text: $draft.body).frame(minHeight: 220).accessibilityLabel("Message body")
                }
                if let error { Text(error).foregroundStyle(Theme.danger) }
            }
            .navigationTitle(draft.replyTo == nil ? "New message" : "Reply")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(sending) }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Send", systemImage: "paperplane") {
                        Task {
                            guard let core = model.core else { return }
                            sending = true
                            defer { sending = false }
                            do {
                                _ = try await core.send(accountId: draft.account, to: splitRecipients(value: draft.to), cc: [], subject: draft.subject, bodyMarkdown: draft.body, replyTo: draft.replyTo)
                                model.notice = "Queued for sending"
                                dismiss()
                                await model.sync()
                            } catch { self.error = error.localizedDescription }
                        }
                    }.disabled(sending || draft.to.isEmpty || draft.account.isEmpty)
                }
            }
            .disabled(sending)
            .interactiveDismissDisabled(sending)
        }
    }
}
