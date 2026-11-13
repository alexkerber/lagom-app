import SwiftUI

/// The desktop compose sheet: From, To, Cc and Subject lines, the body, and Send with a word count.
struct ComposeView: View {
    let model: MailModel
    @State var draft: ComposeDraft
    @State private var sending = false
    @State private var error: String?
    @State private var cancelling = false
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        SheetScaffold(title: draft.title, cancelDisabled: sending, cancel: { cancelling = true }) {
            VStack(spacing: 0) {
                FieldRow(label: "From") {
                    Menu {
                        Picker("From", selection: Binding(get: { draft.account }, set: { account in
                            sending = true
                            Task {
                                defer { sending = false }
                                do {
                                    model.composeDraft = draft
                                    try await model.changeComposeAccount(account)
                                    if let updated = model.composeDraft { draft = updated }
                                } catch { self.error = error.localizedDescription }
                            }
                        })) {
                            ForEach(model.accounts, id: \.id) { Text($0.email).tag($0.id) }
                        }
                    } label: {
                        HStack(spacing: 4) {
                            Text(from).lineLimit(1)
                            if !draft.isReply && !draft.restored { Icon("chevron", size: 14).foregroundStyle(Theme.secondary) }
                        }
                        .foregroundStyle(Theme.ink)
                        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading).contentShape(Rectangle())
                    }
                    .disabled(draft.isReply || draft.restored)
                    .accessibilityLabel("From")
                    .accessibilityValue(from)
                }
                FieldRow(label: "To") { address("To", binding(\.to)) }
                FieldRow(label: "Cc") { address("Cc", binding(\.cc)) }
                FieldRow(label: "Subject") {
                    TextField("Subject", text: binding(\.subject), prompt: Text("")).fontWeight(.semibold).accessibilityLabel("Subject")
                }
                if !draft.attachments.isEmpty {
                    HStack(spacing: 6) {
                        Icon("attachment", size: 16).foregroundStyle(Theme.secondary)
                        Text(draft.attachments.map(\.filename).joined(separator: ", ")).lagomFont(.small).lineLimit(2)
                    }
                    .padding(.vertical, 8).frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(.horizontal, 16)
            TextEditor(text: binding(\.body))
                .lagomFont(.body).lineSpacing(3)
                .scrollContentBackground(.hidden)
                .overlay(alignment: .topLeading) {
                    if draft.body.isEmpty {
                        Text("Message · Markdown supported").lagomFont(.body).foregroundStyle(Theme.secondary)
                            .padding(.top, 8).padding(.leading, 5).allowsHitTesting(false).accessibilityHidden(true)
                    }
                }
                .padding(.horizontal, 12).padding(.vertical, 8)
                .accessibilityLabel("Message body")
        } footer: {
            HouseButton(title: "Send", icon: "send", primary: true) {
                Task {
                    guard model.core != nil else { return }
                    sending = true
                    defer { sending = false }
                    do {
                        model.composeDraft = draft
                        try await model.sendCompose()
                        dismiss()
                        await model.sync()
                    } catch { self.error = error.localizedDescription }
                }
            }
            .disabled(sending || (draft.to.isEmpty && draft.cc.isEmpty) || draft.account.isEmpty)
            if sending { ProgressView().accessibilityLabel("Sending") }
            Spacer(minLength: 0)
            if let error {
                Text(error).lagomFont(.snippet).foregroundStyle(Theme.danger).lineLimit(2)
            } else {
                let words = draft.body.split(whereSeparator: \.isWhitespace).count
                Text(words == 1 ? "1 word" : "\(words) words").lagomFont(.small).monospacedDigit().foregroundStyle(Theme.secondary)
            }
        }
        .onDisappear { model.composeDisappeared(draft) }
        .confirmationDialog("Save draft?", isPresented: $cancelling, titleVisibility: .visible) {
            Button("Keep draft") { close(discard: false) }
            Button("Discard", role: .destructive) { close(discard: true) }
            Button("Cancel", role: .cancel) { }
        }
        .disabled(sending)
        .interactiveDismissDisabled(sending)
    }

    private func close(discard: Bool) {
        sending = true
        Task {
            defer { sending = false }
            do {
                model.composeDraft = draft
                try await model.closeCompose(discard: discard)
                dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }

    private var from: String { model.accounts.first { $0.id == draft.account }?.email ?? "" }

    private func binding(_ field: WritableKeyPath<ComposeDraft, String>) -> Binding<String> {
        Binding(get: { draft[keyPath: field] }, set: { value in
            draft[keyPath: field] = value
            model.composeDraft = draft
        })
    }

    private func address(_ label: String, _ text: Binding<String>) -> some View {
        TextField(label, text: text, prompt: Text(""))
            .textContentType(.emailAddress).keyboardType(.emailAddress)
            .textInputAutocapitalization(.never).autocorrectionDisabled()
            .accessibilityLabel(label)
    }
}
