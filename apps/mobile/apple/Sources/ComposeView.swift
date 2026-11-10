import SwiftUI

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
                TextField("Cc", text: $draft.cc).textContentType(.emailAddress).keyboardType(.emailAddress).textInputAutocapitalization(.never).autocorrectionDisabled()
                TextField("Subject", text: $draft.subject)
                Section("Message · Markdown supported") {
                    TextEditor(text: $draft.body).frame(minHeight: 220).accessibilityLabel("Message body")
                }
                if !draft.attachments.isEmpty {
                    Section("Attachments") {
                        ForEach(draft.attachments.indices, id: \.self) { index in
                            Label(draft.attachments[index].filename, systemImage: "paperclip")
                        }
                    }
                }
                if let error { Text(error).foregroundStyle(Theme.danger) }
            }
            .navigationTitle(draft.title)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(sending) }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Send", systemImage: "paperplane") {
                        Task {
                            guard let core = model.core else { return }
                            sending = true
                            defer { sending = false }
                            do {
                                try await draft.enqueue(using: core)
                                model.notice = "Queued for sending"
                                dismiss()
                                await model.sync()
                            } catch { self.error = error.localizedDescription }
                        }
                    }.disabled(sending || (draft.to.isEmpty && draft.cc.isEmpty) || draft.account.isEmpty)
                }
            }
            .disabled(sending)
            .interactiveDismissDisabled(sending)
        }
    }
}
